//! `atlas-check`: the gate that stops bad art from reaching the renderer.
//!
//! Every problem is reported with a stable `ERR_ATLAS_*` code and a line a human
//! can act on; the binary exits 1 when the list is not empty. The checks exist
//! because each of them is a bug that is invisible in the source art and obvious
//! (and expensive) once the world is running:
//!
//! * a pivot that drifts between frames makes the character bob;
//! * a mirrored direction with its own sheet doubles the texture budget silently;
//! * a page over 2048 px does not upload on the floor tier;
//! * an embedded colour profile makes the browser and the packer disagree;
//! * a dangling frame id is a hole in the animation at runtime.

use std::collections::BTreeMap;
use std::path::Path;

use crate::png;
use crate::schema::{mirror_source, Atlas, DirEntry, Frame, PAGE_MAX, UNIQUE_DIRS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub code: &'static str,
    pub message: String,
}

impl Issue {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Reads `atlas.json` at `path` and validates it together with its pages.
/// An unreadable or unparseable file comes back as a single issue, so callers
/// never have two error channels to handle.
pub fn check_file(path: &Path) -> Vec<Issue> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            return vec![Issue::new(
                "ERR_ATLAS_IO",
                format!("{}: {e}", path.display()),
            )]
        }
    };
    let atlas: Atlas = match serde_json::from_str(&text) {
        Ok(atlas) => atlas,
        Err(e) => {
            return vec![Issue::new(
                "ERR_ATLAS_JSON",
                format!("{}: {e}", path.display()),
            )]
        }
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut issues = check_pages(&atlas, dir);
    issues.extend(check_atlas(&atlas, &page_sizes(&atlas, dir)));
    issues
}

/// Page dimensions read from disk, falling back to the declared `page_sizes`
/// so the structural checks still run when a page file is missing.
fn page_sizes(atlas: &Atlas, dir: &Path) -> Vec<(u32, u32)> {
    atlas
        .pages
        .iter()
        .enumerate()
        .map(|(i, name)| {
            std::fs::read(dir.join(name))
                .ok()
                .and_then(|bytes| png::inspect(&bytes).ok())
                .map(|info| (info.width, info.height))
                .or_else(|| atlas.page_sizes.get(i).map(|[w, h]| (*w, *h)))
                .unwrap_or((PAGE_MAX, PAGE_MAX))
        })
        .collect()
}

/// Checks that only need the files: presence, size, colour profile.
fn check_pages(atlas: &Atlas, dir: &Path) -> Vec<Issue> {
    let mut issues = Vec::new();
    for (i, name) in atlas.pages.iter().enumerate() {
        let path = dir.join(name);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) => {
                issues.push(Issue::new(
                    "ERR_ATLAS_PAGE_MISSING",
                    format!("page {i} '{name}': {e}"),
                ));
                continue;
            }
        };
        let info = match png::inspect(&bytes) {
            Ok(info) => info,
            Err(e) => {
                issues.push(Issue::new(
                    "ERR_ATLAS_PAGE_PNG",
                    format!("page {i} '{name}': {e}"),
                ));
                continue;
            }
        };
        if info.width > PAGE_MAX || info.height > PAGE_MAX {
            issues.push(Issue::new(
                "ERR_ATLAS_PAGE_TOO_BIG",
                format!(
                    "page {i} '{name}' is {}x{}, over the {PAGE_MAX}x{PAGE_MAX} ceiling",
                    info.width, info.height
                ),
            ));
        }
        let profile = info.color_profile_chunks();
        if !profile.is_empty() {
            issues.push(Issue::new(
                "ERR_ATLAS_COLOR_PROFILE",
                format!(
                    "page {i} '{name}' carries {}; pages must be plain sRGB with no embedded profile",
                    profile.join(", ")
                ),
            ));
        }
        if let Some([w, h]) = atlas.page_sizes.get(i) {
            if (*w, *h) != (info.width, info.height) {
                issues.push(Issue::new(
                    "ERR_ATLAS_PAGE_SIZE",
                    format!(
                        "page {i} '{name}': page_sizes says {w}x{h}, the file is {}x{}",
                        info.width, info.height
                    ),
                ));
            }
        }
    }
    if !atlas.page_sizes.is_empty() && atlas.page_sizes.len() != atlas.pages.len() {
        issues.push(Issue::new(
            "ERR_ATLAS_PAGE_SIZE",
            format!(
                "page_sizes has {} entries for {} pages",
                atlas.page_sizes.len(),
                atlas.pages.len()
            ),
        ));
    }
    issues
}

/// Structural checks. `sizes` is the resolved `(w, h)` of each page.
pub fn check_atlas(atlas: &Atlas, sizes: &[(u32, u32)]) -> Vec<Issue> {
    let mut issues = Vec::new();
    if atlas.version != 1 {
        issues.push(Issue::new(
            "ERR_ATLAS_VERSION",
            format!("version {} is not supported (expected 1)", atlas.version),
        ));
    }

    for (id, frame) in &atlas.frames {
        let Some(&(pw, ph)) = sizes.get(frame.page) else {
            issues.push(Issue::new(
                "ERR_ATLAS_PAGE_INDEX",
                format!(
                    "frame '{id}' points at page {}, which does not exist",
                    frame.page
                ),
            ));
            continue;
        };
        if frame.x + frame.w > pw || frame.y + frame.h > ph {
            issues.push(Issue::new(
                "ERR_ATLAS_FRAME_OUT_OF_PAGE",
                format!(
                    "frame '{id}' is {}x{} at ({},{}) on a {pw}x{ph} page",
                    frame.w, frame.h, frame.x, frame.y
                ),
            ));
        }
    }

    for (name, anim) in &atlas.anims {
        if anim.fps == 0 || anim.fps > 30 {
            issues.push(Issue::new(
                "ERR_ATLAS_FPS",
                format!("anim '{name}' runs at {} fps, outside 1..=30", anim.fps),
            ));
        }
        let mut pivots: Vec<(&String, [i32; 2])> = Vec::new();
        for (dir, entry) in anim.dirs.iter() {
            match (mirror_source(dir), entry) {
                // A mirrored direction with its own frames doubles the art and
                // the texture for nothing, and the two sheets drift apart.
                (Some(source_dir), DirEntry::Frames(_)) => issues.push(Issue::new(
                    "ERR_ATLAS_MIRROR_SHEET",
                    format!("anim '{name}': direction {dir} has its own sheet; it must be {{\"mirror_of\":\"{source_dir}\"}}"),
                )),
                (Some(source_dir), DirEntry::Mirror(m)) => {
                    if m.mirror_of != source_dir {
                        issues.push(Issue::new(
                            "ERR_ATLAS_MIRROR_TARGET",
                            format!("anim '{name}': direction {dir} mirrors '{}', expected '{source_dir}'", m.mirror_of),
                        ));
                    }
                }
                (None, DirEntry::Mirror(m)) => issues.push(Issue::new(
                    "ERR_ATLAS_DIR_KIND",
                    format!(
                        "anim '{name}': direction {dir} is one of the {} unique directions and cannot mirror '{}'",
                        UNIQUE_DIRS.len(),
                        m.mirror_of
                    ),
                )),
                (None, DirEntry::Frames(ids)) => {
                    if ids.is_empty() {
                        issues.push(Issue::new(
                            "ERR_ATLAS_EMPTY_DIR",
                            format!("anim '{name}': direction {dir} has no frame"),
                        ));
                    }
                    for id in ids {
                        match atlas.frames.get(id) {
                            Some(frame) => pivots.push((id, frame.pivot)),
                            None => issues.push(Issue::new(
                                "ERR_ATLAS_FRAME_MISSING",
                                format!("anim '{name}', direction {dir}: frame '{id}' is not in the atlas"),
                            )),
                        }
                    }
                }
            }
        }
        if let Some(issue) = pivot_drift(name, &pivots) {
            issues.push(issue);
        }
    }

    for (key, tile) in &atlas.tiles {
        if !atlas.frames.contains_key(&tile.frame) {
            issues.push(Issue::new(
                "ERR_ATLAS_TILE_FRAME",
                format!(
                    "tile '{key}' points at frame '{}', which is not in the atlas",
                    tile.frame
                ),
            ));
        }
        if tile.footprint[0] == 0 || tile.footprint[1] == 0 {
            issues.push(Issue::new(
                "ERR_ATLAS_TILE_FOOTPRINT",
                format!("tile '{key}' has a zero footprint {:?}", tile.footprint),
            ));
        }
    }

    issues
}

/// The ground contact point may move at most 1 px across an animation; more than
/// that and the character visibly bobs against the floor.
fn pivot_drift(anim: &str, pivots: &[(&String, [i32; 2])]) -> Option<Issue> {
    if pivots.len() < 2 {
        return None;
    }
    let (min_x, max_x) = min_max(pivots.iter().map(|(_, p)| p[0]));
    let (min_y, max_y) = min_max(pivots.iter().map(|(_, p)| p[1]));
    let (dx, dy) = (max_x - min_x, max_y - min_y);
    if dx <= 1 && dy <= 1 {
        return None;
    }
    let axis = if dy > dx { 1 } else { 0 };
    let lowest = extreme(pivots, axis, true);
    let highest = extreme(pivots, axis, false);
    Some(Issue::new(
        "ERR_ATLAS_PIVOT_DRIFT",
        format!(
            "anim '{anim}': pivot moves {dx} px in x and {dy} px in y across its frames (max 1); from {lowest} to {highest}"
        ),
    ))
}

/// The frame with the smallest (or largest) pivot on one axis, for the message.
fn extreme(pivots: &[(&String, [i32; 2])], axis: usize, lowest: bool) -> String {
    pivots
        .iter()
        .min_by_key(|(_, p)| if lowest { p[axis] } else { -p[axis] })
        .map(|(id, p)| format!("{id} at {p:?}"))
        .unwrap_or_default()
}

fn min_max(values: impl Iterator<Item = i32>) -> (i32, i32) {
    values.fold((i32::MAX, i32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)))
}

/// Page dimensions taken from the atlas' own `page_sizes`, for callers that hold
/// an `Atlas` in memory and no files.
pub fn declared_page_sizes(atlas: &Atlas) -> Vec<(u32, u32)> {
    atlas.page_sizes.iter().map(|[w, h]| (*w, *h)).collect()
}

/// Frames that no animation and no tile reference. Not an error: art gets packed
/// before it is wired up. Reported by `atlas-check` as a note.
pub fn orphan_frames(atlas: &Atlas) -> Vec<&String> {
    let mut used: BTreeMap<&String, bool> = atlas.frames.keys().map(|k| (k, false)).collect();
    for anim in atlas.anims.values() {
        for (_, entry) in anim.dirs.iter() {
            if let DirEntry::Frames(ids) = entry {
                for id in ids {
                    if let Some(flag) = used.get_mut(id) {
                        *flag = true;
                    }
                }
            }
        }
    }
    for tile in atlas.tiles.values() {
        if let Some(flag) = used.get_mut(&tile.frame) {
            *flag = true;
        }
    }
    used.into_iter()
        .filter(|(_, seen)| !seen)
        .map(|(id, _)| id)
        .collect()
}

/// Convenience for tests and for callers that build an atlas in memory.
pub fn frame_fits(frame: &Frame, page: (u32, u32)) -> bool {
    frame.x + frame.w <= page.0 && frame.y + frame.h <= page.1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Anim, Dirs, Mirror, Tile};

    fn frame(x: u32, y: u32, pivot: [i32; 2]) -> Frame {
        Frame {
            page: 0,
            x,
            y,
            w: 8,
            h: 8,
            pivot,
            z_base: 0,
        }
    }

    fn atlas_with_walk(pivots: [[i32; 2]; 3]) -> Atlas {
        let mut atlas = Atlas::empty();
        atlas.pages.push("omni-0.png".into());
        atlas.page_sizes.push([64, 64]);
        let mut s_ids = Vec::new();
        for (i, pivot) in pivots.iter().enumerate() {
            let id = format!("omni/walk/S/{i}");
            atlas
                .frames
                .insert(id.clone(), frame(i as u32 * 10, 0, *pivot));
            s_ids.push(id);
        }
        for dir in ["SW", "W", "NW", "N"] {
            let id = format!("omni/walk/{dir}/0");
            atlas.frames.insert(id, frame(0, 10, pivots[0]));
        }
        atlas.anims.insert(
            "omni/walk".into(),
            Anim {
                fps: 10,
                looping: true,
                dirs: Dirs::from_unique(
                    s_ids,
                    vec!["omni/walk/SW/0".into()],
                    vec!["omni/walk/W/0".into()],
                    vec!["omni/walk/NW/0".into()],
                    vec!["omni/walk/N/0".into()],
                ),
            },
        );
        atlas
    }

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    #[test]
    fn a_clean_atlas_reports_nothing() {
        let atlas = atlas_with_walk([[4, 7], [4, 7], [5, 7]]);
        let issues = check_atlas(&atlas, &declared_page_sizes(&atlas));
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn pivot_drift_over_one_pixel_fails() {
        let atlas = atlas_with_walk([[4, 7], [4, 7], [4, 10]]);
        let issues = check_atlas(&atlas, &declared_page_sizes(&atlas));
        assert_eq!(codes(&issues), vec!["ERR_ATLAS_PIVOT_DRIFT"]);
        assert!(
            issues[0].message.contains("3 px in y"),
            "{}",
            issues[0].message
        );
    }

    #[test]
    fn a_mirrored_direction_with_its_own_sheet_fails() {
        let mut atlas = atlas_with_walk([[4, 7]; 3]);
        atlas
            .frames
            .insert("omni/walk/E/0".into(), frame(20, 20, [4, 7]));
        let anim = atlas.anims.get_mut("omni/walk").expect("anim");
        anim.dirs.e = DirEntry::Frames(vec!["omni/walk/E/0".into()]);
        let issues = check_atlas(&atlas, &declared_page_sizes(&atlas));
        assert_eq!(codes(&issues), vec!["ERR_ATLAS_MIRROR_SHEET"]);
        assert!(issues[0].message.contains("mirror_of"));
    }

    #[test]
    fn a_mirror_pointing_at_the_wrong_direction_fails() {
        let mut atlas = atlas_with_walk([[4, 7]; 3]);
        atlas.anims.get_mut("omni/walk").expect("anim").dirs.ne = DirEntry::Mirror(Mirror {
            mirror_of: "W".into(),
        });
        assert_eq!(
            codes(&check_atlas(&atlas, &declared_page_sizes(&atlas))),
            vec!["ERR_ATLAS_MIRROR_TARGET"]
        );
    }

    #[test]
    fn a_unique_direction_declared_as_a_mirror_fails() {
        let mut atlas = atlas_with_walk([[4, 7]; 3]);
        atlas.anims.get_mut("omni/walk").expect("anim").dirs.n = DirEntry::Mirror(Mirror {
            mirror_of: "NW".into(),
        });
        assert_eq!(
            codes(&check_atlas(&atlas, &declared_page_sizes(&atlas))),
            vec!["ERR_ATLAS_DIR_KIND"]
        );
    }

    #[test]
    fn a_frame_outside_the_page_and_a_bad_page_index_fail() {
        let mut atlas = atlas_with_walk([[4, 7]; 3]);
        atlas.frames.insert(
            "omni/walk/S/9".into(),
            Frame {
                page: 0,
                x: 60,
                y: 60,
                w: 8,
                h: 8,
                pivot: [4, 7],
                z_base: 0,
            },
        );
        atlas.frames.insert(
            "omni/walk/S/8".into(),
            Frame {
                page: 3,
                x: 0,
                y: 0,
                w: 8,
                h: 8,
                pivot: [4, 7],
                z_base: 0,
            },
        );
        let issues = check_atlas(&atlas, &declared_page_sizes(&atlas));
        assert_eq!(
            codes(&issues),
            vec!["ERR_ATLAS_PAGE_INDEX", "ERR_ATLAS_FRAME_OUT_OF_PAGE"]
        );
    }

    #[test]
    fn a_dangling_frame_reference_fails() {
        let mut atlas = atlas_with_walk([[4, 7]; 3]);
        atlas.anims.get_mut("omni/walk").expect("anim").dirs.n =
            DirEntry::Frames(vec!["omni/walk/N/does-not-exist".into()]);
        assert_eq!(
            codes(&check_atlas(&atlas, &declared_page_sizes(&atlas))),
            vec!["ERR_ATLAS_FRAME_MISSING"]
        );
    }

    #[test]
    fn a_tile_pointing_at_nothing_fails() {
        let mut atlas = atlas_with_walk([[4, 7]; 3]);
        atlas.tiles.insert(
            "floor/wood".into(),
            Tile {
                frame: "casa-v1/floor/wood".into(),
                height: 0,
                occludes: false,
                footprint: [1, 1],
                walkable: true,
            },
        );
        let issues = check_atlas(&atlas, &declared_page_sizes(&atlas));
        assert_eq!(codes(&issues), vec!["ERR_ATLAS_TILE_FRAME"]);
    }

    #[test]
    fn orphan_frames_are_listed_but_are_not_issues() {
        let mut atlas = atlas_with_walk([[4, 7]; 3]);
        atlas
            .frames
            .insert("omni/loose".into(), frame(30, 30, [4, 7]));
        assert_eq!(orphan_frames(&atlas), vec![&"omni/loose".to_string()]);
        assert!(check_atlas(&atlas, &declared_page_sizes(&atlas)).is_empty());
    }

    #[test]
    fn frame_fits_guards_both_axes() {
        let f = frame(60, 0, [0, 0]);
        assert!(!frame_fits(&f, (64, 64)));
        assert!(frame_fits(&f, (68, 64)));
    }
}
