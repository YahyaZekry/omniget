//! `house-check`: the gate for the world's content.
//!
//! Three files have to agree before the house is playable:
//!
//! * `static/world/tiles/casa-v1/atlas.json` — the art and what each tile is;
//! * `static/world/house-v1.json` — the [`MapDef`] the simulation loads;
//! * `static/world/routines/*.json` — the day of each role of the roster.
//!
//! The simulation is forgiving on purpose: `World` corrects a spawn that lands
//! on a bed instead of refusing it, and `route` walks to the nearest passable
//! tile when a decision names a wall. That is right at runtime and wrong at
//! authoring time, where a silently corrected mistake is a mistake nobody
//! sees. So every check here is strict: a spawn on furniture, an unreachable
//! slot, a routine that sends an agent into a wall and a palette entry that
//! disagrees with the atlas are all failures, each with a stable `ERR_HOUSE_*`
//! code.
//!
//! Everything below is a pure function of bytes already read from disk, except
//! [`check_files`], which reads them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use omniget_world::ents::object::kind_leaf;
use omniget_world::map::{Map, MapDef, SlotKind, Tile, TileDef};
use omniget_world::sim::{Decision, Routine, RoutineEntry, MAX_SAY_BYTES, MINUTES_PER_DAY};
use omniget_world::{AStar, Grid, ObjectId, World};

use crate::schema::Atlas;

/// Version of the `routines/*.json` format this build understands.
pub const ROUTINE_FORMAT_VERSION: u32 = 1;

/// Seed used to build the throwaway `World` the reachability checks walk in.
/// The map is static, so the number never shows up in a result.
const CHECK_SEED: u64 = 0x686f_7573_6531;

/// Kinds the house must ship with, whatever else the player later swaps in.
/// They are the ones the product promises: somewhere to sleep, somewhere to
/// work, and the screen the stream is cast to (plan §9.1).
pub const REQUIRED_KINDS: [&str; 3] = ["object/bed", "object/workbench", "object/tv"];

/// Objects a `table` slot may stand on.
const SURFACE_KINDS: [&str; 3] = ["table", "workbench", "chest"];

/// One problem found. `code` is stable; `detail` says where.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Issue {
    pub code: &'static str,
    pub detail: String,
}

impl Issue {
    fn new(code: &'static str, detail: impl Into<String>) -> Issue {
        Issue {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

// --- the routine file format -------------------------------------------------

/// What a routine entry tells the agent to do. `Decision::Wave` is missing on
/// purpose: it names a runtime `EntId`, which a file cannot know.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "lowercase")]
pub enum RoutineAct {
    Goto { tile: [i32; 2] },
    Sit { object: u32 },
    Work { object: u32 },
    Sleep,
    Say { text: String },
    Idle,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RoutineEntryDef {
    /// `HH:MM` of the game day.
    pub at: String,
    #[serde(flatten)]
    pub act: RoutineAct,
}

/// `static/world/routines/<role>.json`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RoutineDef {
    pub version: u32,
    pub id: String,
    /// Roster role this routine is the default for: `coordinator`, `worker`,
    /// `advisor` or `custom`.
    pub role: String,
    /// `id` of the map the tiles and object ids belong to.
    pub map: String,
    pub entries: Vec<RoutineEntryDef>,
}

/// `"07:30"` into a minute of the game day. Rejects anything else, including
/// `"7:30"`, so the files stay uniform.
pub fn parse_hm(s: &str) -> Option<u16> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// Minute of the game day back into `HH:MM`, for messages.
pub fn format_hm(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// Turn a parsed file into the `Routine` the simulation takes. Fails on the
/// same things [`check_routine`] reports, so a caller that skipped the check
/// still cannot build a broken routine.
pub fn routine_of(def: &RoutineDef) -> Result<Routine, Issue> {
    let mut entries = Vec::with_capacity(def.entries.len());
    for e in &def.entries {
        let Some(minute) = parse_hm(&e.at) else {
            return Err(Issue::new(
                "ERR_HOUSE_ROUTINE_TIME",
                format!("{}: {:?} is not HH:MM", def.id, e.at),
            ));
        };
        let decision = match &e.act {
            RoutineAct::Goto { tile } => Decision::GoTo(Tile::new(tile[0], tile[1])),
            RoutineAct::Sit { object } => Decision::Sit(ObjectId(*object)),
            RoutineAct::Work { object } => Decision::Work(ObjectId(*object)),
            RoutineAct::Sleep => Decision::Sleep,
            RoutineAct::Say { text } => Decision::Say(text.clone()),
            RoutineAct::Idle => Decision::Idle,
        };
        entries.push(RoutineEntry { minute, decision });
    }
    Ok(Routine::new(entries))
}

// --- the checks --------------------------------------------------------------

/// Everything the checks need, built once.
struct Scene {
    world: World,
    astar: AStar,
    spawn: Tile,
}

impl Scene {
    fn new(def: &MapDef) -> Result<Scene, Issue> {
        let world = World::new(CHECK_SEED, def.clone())
            .map_err(|e| Issue::new("ERR_HOUSE_MAP_INVALID", e.to_string()))?;
        let spawn = world.map().marker("spawn").unwrap_or(Tile::new(0, 0));
        Ok(Scene {
            world,
            astar: AStar::new(),
            spawn,
        })
    }

    fn grid(&self) -> &Grid {
        self.world.grid()
    }

    /// Is there a walk from the spawn to this tile, under the very rules the
    /// tick walks by?
    fn reachable(&mut self, to: Tile) -> bool {
        if self.spawn == to {
            return true;
        }
        let mut out = Vec::new();
        let grid = self.world.grid();
        self.astar.find(grid, self.spawn, to, &mut out)
    }
}

/// Check a map against the atlas it draws with and the routines written for
/// it. An empty result is a pass.
pub fn check(def: &MapDef, atlas: &Atlas, routines: &[(String, RoutineDef)]) -> Vec<Issue> {
    let mut out = Vec::new();
    check_palette(def, atlas, &mut out);

    let mut scene = match Scene::new(def) {
        Ok(s) => s,
        Err(e) => {
            // Nothing below can run without a loaded map.
            out.push(e);
            return out;
        }
    };

    check_required_kinds(def, &mut out);
    check_objects(def, &mut scene, &mut out);
    check_slots(def, &mut scene, &mut out);
    check_markers(def, &mut scene, &mut out);
    check_rooms(def, &mut scene, &mut out);
    for (name, routine) in routines {
        check_routine(name, routine, def, &mut scene, &mut out);
    }
    out
}

fn palette_of<'a>(def: &'a MapDef, key: &str) -> Option<&'a TileDef> {
    def.palette.iter().find(|t| t.key == key)
}

fn footprint_of(def: &MapDef, key: &str) -> [u8; 2] {
    palette_of(def, key).map(|t| t.footprint).unwrap_or([1, 1])
}

/// Every palette entry must exist in the atlas and say exactly what the atlas
/// says. A map that disagrees with the art draws a bed an agent can walk
/// through.
fn check_palette(def: &MapDef, atlas: &Atlas, out: &mut Vec<Issue>) {
    let mut seen = BTreeSet::new();
    for t in &def.palette {
        if !seen.insert(t.key.clone()) {
            out.push(Issue::new(
                "ERR_HOUSE_TILE_DUPLICATE",
                format!("palette lists {} twice", t.key),
            ));
        }
        let Some(a) = atlas.tiles.get(&t.key) else {
            out.push(Issue::new(
                "ERR_HOUSE_TILE_UNKNOWN",
                format!("{} is not a tile of the atlas", t.key),
            ));
            continue;
        };
        let mismatch = a.height != u32::from(t.height)
            || a.occludes != t.occludes
            || a.walkable != t.walkable
            || a.footprint != [u32::from(t.footprint[0]), u32::from(t.footprint[1])];
        if mismatch {
            out.push(Issue::new(
                "ERR_HOUSE_TILE_MISMATCH",
                format!(
                    "{}: map says height {} occludes {} walkable {} footprint {:?}, atlas says \
                     height {} occludes {} walkable {} footprint {:?}",
                    t.key,
                    t.height,
                    t.occludes,
                    t.walkable,
                    t.footprint,
                    a.height,
                    a.occludes,
                    a.walkable,
                    a.footprint
                ),
            ));
        }
    }
}

fn check_required_kinds(def: &MapDef, out: &mut Vec<Issue>) {
    for want in REQUIRED_KINDS {
        if !def.objects.iter().any(|o| o.kind == want) {
            out.push(Issue::new(
                "ERR_HOUSE_MISSING_KIND",
                format!("the house ships with no {want}"),
            ));
        }
    }
}

/// Objects: known kind, inside the map, not buried in a wall, and usable —
/// the tile an agent stands on to sit, sleep or work has to be free.
fn check_objects(def: &MapDef, scene: &mut Scene, out: &mut Vec<Issue>) {
    let slot_kind: BTreeMap<&str, SlotKind> =
        def.slots.iter().map(|s| (s.id.as_str(), s.kind)).collect();

    for o in &def.objects {
        let Some(tile_def) = palette_of(def, &o.kind) else {
            out.push(Issue::new(
                "ERR_HOUSE_OBJECT_KIND",
                format!(
                    "object {} is a {}, which is not in the palette",
                    o.id, o.kind
                ),
            ));
            continue;
        };
        if !o.kind.starts_with("object/") {
            out.push(Issue::new(
                "ERR_HOUSE_OBJECT_KIND",
                format!(
                    "object {} is a {}, which is not an object tile",
                    o.id, o.kind
                ),
            ));
        }
        let kind = match o.slot.as_deref().and_then(|s| slot_kind.get(s)) {
            Some(k) => *k,
            None => {
                out.push(Issue::new(
                    "ERR_HOUSE_OBJECT_NO_SLOT",
                    format!(
                        "object {} ({}) sits in no slot, so the player can never move it",
                        o.id, o.kind
                    ),
                ));
                SlotKind::Floor
            }
        };

        let fp = tile_def.footprint;
        let map = scene.world.map();
        for t in footprint_tiles(o.tile, fp) {
            if !map.contains(t) {
                out.push(Issue::new(
                    "ERR_HOUSE_OBJECT_OUTSIDE",
                    format!("object {} covers ({}, {}), outside the map", o.id, t.x, t.y),
                ));
                continue;
            }
            // `Map::walkable` is the static layers only: a tile that is not
            // walkable before any object is placed holds a wall.
            if kind != SlotKind::Wall && !map.walkable(t) {
                out.push(Issue::new(
                    "ERR_HOUSE_OBJECT_IN_WALL",
                    format!(
                        "object {} ({}) covers ({}, {}), which is wall or has no floor",
                        o.id, o.kind, t.x, t.y
                    ),
                ));
            }
        }

        if kind == SlotKind::Wall && !touches_wall(scene.world.map(), o.tile) {
            out.push(Issue::new(
                "ERR_HOUSE_WALL_SLOT_ADRIFT",
                format!(
                    "object {} ({}) is in a wall slot but ({}, {}) is not on or beside a wall",
                    o.id, o.kind, o.tile.x, o.tile.y
                ),
            ));
        }

        // Sitting, sleeping and working all route to `Object::approach()`, the
        // tile south of the footprint. If another object stands there the
        // agent is sent to whatever `nearest_passable` finds instead, which is
        // how furniture ends up used from the wrong side.
        let leaf = kind_leaf(&o.kind);
        if matches!(
            leaf,
            "chair" | "bed" | "sofa" | "stool" | "workbench" | "desk" | "bookshelf"
        ) {
            let approach = Tile::new(o.tile.x, o.tile.y + i32::from(fp[1].max(1)));
            if !scene.grid().passable(approach) {
                out.push(Issue::new(
                    "ERR_HOUSE_APPROACH_BLOCKED",
                    format!(
                        "object {} ({}) is used from ({}, {}), which is blocked",
                        o.id, o.kind, approach.x, approach.y
                    ),
                ));
            } else if !scene.reachable(approach) {
                out.push(Issue::new(
                    "ERR_HOUSE_UNREACHABLE",
                    format!(
                        "object {} ({}) is used from ({}, {}), which no walk from the spawn reaches",
                        o.id, o.kind, approach.x, approach.y
                    ),
                ));
            }
        }
    }
}

/// Slots: a real catalogue, a default that the slot itself accepts, an object
/// in it, a surface under a `table` slot, and a way to walk up to it.
fn check_slots(def: &MapDef, scene: &mut Scene, out: &mut Vec<Issue>) {
    let floor_slots: BTreeMap<(i32, i32), &str> = def
        .slots
        .iter()
        .filter(|s| s.kind == SlotKind::Floor)
        .filter_map(|s| s.default.as_deref().map(|d| ((s.tile.x, s.tile.y), d)))
        .collect();

    for s in &def.slots {
        if s.accepts.is_empty() {
            out.push(Issue::new(
                "ERR_HOUSE_SLOT_NO_CATALOGUE",
                format!("slot {} accepts nothing, so nothing can be put in it", s.id),
            ));
        }
        for key in &s.accepts {
            if palette_of(def, key).is_none() {
                out.push(Issue::new(
                    "ERR_HOUSE_SLOT_ACCEPTS_UNKNOWN",
                    format!("slot {} accepts {key}, which is not in the palette", s.id),
                ));
            }
        }
        match &s.default {
            None => out.push(Issue::new(
                "ERR_HOUSE_SLOT_NO_DEFAULT",
                format!("slot {} has no default object", s.id),
            )),
            Some(d) => {
                if !s.accepts.is_empty() && !s.accepts.contains(d) {
                    out.push(Issue::new(
                        "ERR_HOUSE_SLOT_REJECTS_DEFAULT",
                        format!("slot {} defaults to {d}, which it does not accept", s.id),
                    ));
                }
            }
        }

        let here: Vec<_> = def
            .objects
            .iter()
            .filter(|o| o.slot.as_deref() == Some(s.id.as_str()))
            .collect();
        match here.len() {
            0 => out.push(Issue::new(
                "ERR_HOUSE_SLOT_EMPTY",
                format!("slot {} has a default but no object in it", s.id),
            )),
            1 => {
                let o = here[0];
                if o.tile != s.tile {
                    out.push(Issue::new(
                        "ERR_HOUSE_SLOT_OFF_TILE",
                        format!(
                            "object {} sits at ({}, {}) but slot {} is at ({}, {})",
                            o.id, o.tile.x, o.tile.y, s.id, s.tile.x, s.tile.y
                        ),
                    ));
                }
                if !s.accepts.is_empty() && !s.accepts.contains(&o.kind) {
                    out.push(Issue::new(
                        "ERR_HOUSE_SLOT_REJECTS_OBJECT",
                        format!("slot {} holds a {}, which it does not accept", s.id, o.kind),
                    ));
                }
            }
            n => out.push(Issue::new(
                "ERR_HOUSE_SLOT_CROWDED",
                format!("slot {} holds {n} objects", s.id),
            )),
        }

        if s.kind == SlotKind::Table {
            let under = floor_slots.get(&(s.tile.x, s.tile.y)).copied();
            let ok = under.is_some_and(|k| SURFACE_KINDS.contains(&kind_leaf(k)));
            if !ok {
                out.push(Issue::new(
                    "ERR_HOUSE_TABLE_SLOT_NO_SURFACE",
                    format!(
                        "table slot {} at ({}, {}) has no table, workbench or chest under it",
                        s.id, s.tile.x, s.tile.y
                    ),
                ));
            }
        }

        // A slot nobody can walk up to cannot be edited in the world.
        let fp = s
            .default
            .as_deref()
            .map(|k| footprint_of(def, k))
            .unwrap_or([1, 1]);
        let ring = approach_ring(s.tile, fp);
        let standable: Vec<Tile> = ring
            .into_iter()
            .filter(|t| scene.grid().passable(*t))
            .collect();
        if standable.is_empty() {
            out.push(Issue::new(
                "ERR_HOUSE_SLOT_WALLED_IN",
                format!("slot {} has no free tile beside it", s.id),
            ));
        } else if !standable.into_iter().any(|t| scene.reachable(t)) {
            out.push(Issue::new(
                "ERR_HOUSE_UNREACHABLE",
                format!("no walk from the spawn reaches slot {}", s.id),
            ));
        }
    }
}

/// Markers: the two the app looks up by name have to be there, and every
/// marker has to name a tile an agent can actually stand on. `World` silently
/// moves a spawn that lands on a bed to the nearest free tile, so this is the
/// only place that mistake is ever visible.
fn check_markers(def: &MapDef, scene: &mut Scene, out: &mut Vec<Issue>) {
    let mut seen = BTreeSet::new();
    for m in &def.markers {
        if !seen.insert(m.name.clone()) {
            out.push(Issue::new(
                "ERR_HOUSE_MARKER_DUPLICATE",
                format!("marker {} is declared twice", m.name),
            ));
        }
        if !scene.grid().passable(m.tile) {
            out.push(Issue::new(
                "ERR_HOUSE_MARKER_BLOCKED",
                format!(
                    "marker {} is at ({}, {}), which is wall or furniture",
                    m.name, m.tile.x, m.tile.y
                ),
            ));
            continue;
        }
        if !scene.reachable(m.tile) {
            out.push(Issue::new(
                "ERR_HOUSE_UNREACHABLE",
                format!("no walk from the spawn reaches marker {}", m.name),
            ));
        }
    }
    for want in ["spawn", "front-door"] {
        if !def.markers.iter().any(|m| m.name == want) {
            out.push(Issue::new(
                "ERR_HOUSE_MARKER_MISSING",
                format!("there is no {want} marker"),
            ));
        }
    }
}

/// Rooms: inside the map, not overlapping, and with a centre an agent can
/// stand on, because `RoomDef::centre` is where "go to the kitchen" aims.
fn check_rooms(def: &MapDef, scene: &mut Scene, out: &mut Vec<Issue>) {
    for (i, r) in def.rooms.iter().enumerate() {
        let [x, y, w, h] = r.rect;
        if w <= 0 || h <= 0 {
            out.push(Issue::new(
                "ERR_HOUSE_ROOM_EMPTY",
                format!("room {} is {w}x{h}", r.id),
            ));
            continue;
        }
        let map = scene.world.map();
        let inside = map.contains(Tile::new(x, y)) && map.contains(Tile::new(x + w - 1, y + h - 1));
        if !inside {
            out.push(Issue::new(
                "ERR_HOUSE_ROOM_OUTSIDE",
                format!("room {} leaves the map", r.id),
            ));
        }
        for other in &def.rooms[i + 1..] {
            if overlaps(r.rect, other.rect) {
                out.push(Issue::new(
                    "ERR_HOUSE_ROOM_OVERLAP",
                    format!("rooms {} and {} overlap", r.id, other.id),
                ));
            }
        }
        let centre = Tile::new(x + w / 2, y + h / 2);
        if !scene.grid().passable(centre) {
            out.push(Issue::new(
                "ERR_HOUSE_ROOM_CENTRE_BLOCKED",
                format!(
                    "room {} aims at ({}, {}), which is wall or furniture",
                    r.id, centre.x, centre.y
                ),
            ));
        } else if !scene.reachable(centre) {
            out.push(Issue::new(
                "ERR_HOUSE_UNREACHABLE",
                format!("no walk from the spawn reaches room {}", r.id),
            ));
        }
    }
}

/// A routine: the right format version, the right map, times that parse,
/// objects that exist and can be used the way the entry uses them, and tiles
/// an agent can both stand on and walk to.
fn check_routine(
    name: &str,
    def: &RoutineDef,
    map: &MapDef,
    scene: &mut Scene,
    out: &mut Vec<Issue>,
) {
    if def.version != ROUTINE_FORMAT_VERSION {
        out.push(Issue::new(
            "ERR_HOUSE_ROUTINE_VERSION",
            format!(
                "{name}: version {} is not {ROUTINE_FORMAT_VERSION}",
                def.version
            ),
        ));
    }
    if def.map != map.id {
        out.push(Issue::new(
            "ERR_HOUSE_ROUTINE_MAP",
            format!("{name}: written for map {}, not {}", def.map, map.id),
        ));
    }
    if def.entries.is_empty() {
        out.push(Issue::new(
            "ERR_HOUSE_ROUTINE_EMPTY",
            format!("{name}: no entries, so the agent never does anything"),
        ));
    }

    let mut minutes = BTreeSet::new();
    let mut sleeps = false;
    for e in &def.entries {
        let Some(minute) = parse_hm(&e.at) else {
            out.push(Issue::new(
                "ERR_HOUSE_ROUTINE_TIME",
                format!("{name}: {:?} is not HH:MM", e.at),
            ));
            continue;
        };
        if u64::from(minute) >= MINUTES_PER_DAY {
            out.push(Issue::new(
                "ERR_HOUSE_ROUTINE_TIME",
                format!("{name}: {} is past the end of the day", e.at),
            ));
        }
        if !minutes.insert(minute) {
            out.push(Issue::new(
                "ERR_HOUSE_ROUTINE_COLLISION",
                format!("{name}: two entries at {}", e.at),
            ));
        }
        match &e.act {
            RoutineAct::Goto { tile } => {
                let t = Tile::new(tile[0], tile[1]);
                if !scene.grid().passable(t) {
                    out.push(Issue::new(
                        "ERR_HOUSE_ROUTINE_TILE",
                        format!(
                            "{name}: {} sends the agent to ({}, {}), which is wall or furniture",
                            e.at, t.x, t.y
                        ),
                    ));
                } else if !scene.reachable(t) {
                    out.push(Issue::new(
                        "ERR_HOUSE_ROUTINE_UNREACHABLE",
                        format!(
                            "{name}: {} sends the agent to ({}, {}), which no walk reaches",
                            e.at, t.x, t.y
                        ),
                    ));
                }
            }
            RoutineAct::Sit { object } => check_routine_object(
                name,
                &e.at,
                *object,
                map,
                &["chair", "bed", "sofa", "stool"],
                out,
            ),
            RoutineAct::Work { object } => check_routine_object(
                name,
                &e.at,
                *object,
                map,
                &["workbench", "desk", "bookshelf"],
                out,
            ),
            RoutineAct::Say { text } => {
                if text.len() > MAX_SAY_BYTES {
                    out.push(Issue::new(
                        "ERR_HOUSE_ROUTINE_SAY",
                        format!(
                            "{name}: {} says {} bytes, the limit is {MAX_SAY_BYTES}",
                            e.at,
                            text.len()
                        ),
                    ));
                }
            }
            RoutineAct::Sleep => sleeps = true,
            RoutineAct::Idle => {}
        }
    }

    if !def.entries.is_empty() && !sleeps {
        out.push(Issue::new(
            "ERR_HOUSE_ROUTINE_NO_SLEEP",
            format!("{name}: no sleep entry, so the agent never goes to bed"),
        ));
    }

    // The file has to survive the trip into the type the simulation takes.
    match routine_of(def) {
        Ok(r) => {
            if r.len() != def.entries.len() {
                out.push(Issue::new(
                    "ERR_HOUSE_ROUTINE_LOST_ENTRY",
                    format!("{name}: {} entries became {}", def.entries.len(), r.len()),
                ));
            }
        }
        Err(e) => out.push(e),
    }
}

fn check_routine_object(
    name: &str,
    at: &str,
    object: u32,
    map: &MapDef,
    leaves: &[&str],
    out: &mut Vec<Issue>,
) {
    let Some(o) = map.objects.iter().find(|o| o.id == object) else {
        out.push(Issue::new(
            "ERR_HOUSE_ROUTINE_OBJECT",
            format!("{name}: {at} names object {object}, which the map does not have"),
        ));
        return;
    };
    if !leaves.contains(&kind_leaf(&o.kind)) {
        out.push(Issue::new(
            "ERR_HOUSE_ROUTINE_OBJECT_KIND",
            format!(
                "{name}: {at} uses object {object}, a {}, which is not one of {leaves:?}",
                o.kind
            ),
        ));
    }
}

// --- geometry helpers --------------------------------------------------------

fn footprint_tiles(at: Tile, fp: [u8; 2]) -> Vec<Tile> {
    let w = i32::from(fp[0].max(1));
    let d = i32::from(fp[1].max(1));
    let mut out = Vec::with_capacity((w * d) as usize);
    for dy in 0..d {
        for dx in 0..w {
            out.push(Tile::new(at.x + dx, at.y + dy));
        }
    }
    out
}

/// The tiles around a footprint, plus the footprint itself: where an agent
/// could stand to reach what sits there.
fn approach_ring(at: Tile, fp: [u8; 2]) -> Vec<Tile> {
    let w = i32::from(fp[0].max(1));
    let d = i32::from(fp[1].max(1));
    let mut out = Vec::new();
    for dy in -1..=d {
        for dx in -1..=w {
            out.push(Tile::new(at.x + dx, at.y + dy));
        }
    }
    out
}

/// Is this tile a wall, or next to one? A wall-mounted object may hang on the
/// wall itself or stand against it.
fn touches_wall(map: &Map, t: Tile) -> bool {
    for dy in -1..=1 {
        for dx in -1..=1 {
            let c = Tile::new(t.x + dx, t.y + dy);
            if map.contains(c) && !map.walkable(c) {
                return true;
            }
        }
    }
    false
}

fn overlaps(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

// --- reading from disk -------------------------------------------------------

/// Read the three inputs and check them. Every failure to read is an issue of
/// its own, so the tool says what it could not open instead of panicking.
pub fn check_files(map_path: &Path, atlas_path: &Path, routines_dir: &Path) -> Vec<Issue> {
    let map_text = match std::fs::read_to_string(map_path) {
        Ok(t) => t,
        Err(e) => {
            return vec![Issue::new(
                "ERR_HOUSE_MAP_READ",
                format!("{}: {e}", map_path.display()),
            )]
        }
    };
    let def: MapDef = match serde_json::from_str(&map_text) {
        Ok(d) => d,
        Err(e) => {
            return vec![Issue::new(
                "ERR_HOUSE_MAP_PARSE",
                format!("{}: {e}", map_path.display()),
            )]
        }
    };
    let atlas_text = match std::fs::read_to_string(atlas_path) {
        Ok(t) => t,
        Err(e) => {
            return vec![Issue::new(
                "ERR_HOUSE_ATLAS_READ",
                format!("{}: {e}", atlas_path.display()),
            )]
        }
    };
    let atlas: Atlas = match serde_json::from_str(&atlas_text) {
        Ok(a) => a,
        Err(e) => {
            return vec![Issue::new(
                "ERR_HOUSE_ATLAS_PARSE",
                format!("{}: {e}", atlas_path.display()),
            )]
        }
    };

    let mut out = Vec::new();
    // `MapDef::atlas` is relative to `static/`; the caller passes a path on
    // disk. Comparing the tail is enough to catch a map pointing at art it is
    // not being checked against.
    if !atlas_path.ends_with(Path::new(&def.atlas)) {
        out.push(Issue::new(
            "ERR_HOUSE_ATLAS_PATH",
            format!(
                "the map draws with {}, but {} was checked",
                def.atlas,
                atlas_path.display()
            ),
        ));
    }

    let mut routines: Vec<(String, RoutineDef)> = Vec::new();
    match read_routines(routines_dir) {
        Ok(found) => {
            if found.is_empty() {
                out.push(Issue::new(
                    "ERR_HOUSE_ROUTINES_MISSING",
                    format!("{} holds no routine", routines_dir.display()),
                ));
            }
            for (name, text) in found {
                match serde_json::from_str::<RoutineDef>(&text) {
                    Ok(r) => routines.push((name, r)),
                    Err(e) => out.push(Issue::new(
                        "ERR_HOUSE_ROUTINE_PARSE",
                        format!("{name}: {e}"),
                    )),
                }
            }
        }
        Err(e) => out.push(Issue::new(
            "ERR_HOUSE_ROUTINES_READ",
            format!("{}: {e}", routines_dir.display()),
        )),
    }

    out.extend(check(&def, &atlas, &routines));
    out
}

/// Every `*.json` in the directory, sorted by name so the report is the same
/// on every machine.
fn read_routines(dir: &Path) -> std::io::Result<Vec<(String, String)>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            paths.push(path);
        }
    }
    paths.sort();
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        out.push((name, std::fs::read_to_string(&p)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniget_world::map::{MarkerDef, ObjectDef, RoomDef, SlotDef};

    const HOUSE: &str = include_str!("../../../static/world/house-v1.json");
    const ATLAS: &str = include_str!("../../../static/world/tiles/casa-v1/atlas.json");
    const COORDINATOR: &str = include_str!("../../../static/world/routines/coordinator.json");
    const WORKER: &str = include_str!("../../../static/world/routines/worker.json");
    const ADVISOR: &str = include_str!("../../../static/world/routines/advisor.json");
    const OMNI: &str = include_str!("../../../static/world/routines/omni.json");

    fn house() -> MapDef {
        serde_json::from_str(HOUSE).expect("house-v1.json parses as a MapDef")
    }

    fn atlas() -> Atlas {
        serde_json::from_str(ATLAS).expect("the casa-v1 atlas parses")
    }

    fn routines() -> Vec<(String, RoutineDef)> {
        [
            ("advisor.json", ADVISOR),
            ("coordinator.json", COORDINATOR),
            ("omni.json", OMNI),
            ("worker.json", WORKER),
        ]
        .into_iter()
        .map(|(n, t)| {
            (
                n.to_string(),
                serde_json::from_str(t).unwrap_or_else(|e| panic!("{n}: {e}")),
            )
        })
        .collect()
    }

    fn codes(issues: &[Issue]) -> Vec<&'static str> {
        issues.iter().map(|i| i.code).collect()
    }

    #[test]
    fn the_real_house_passes() {
        let issues = check(&house(), &atlas(), &routines());
        assert!(issues.is_empty(), "{:#?}", issues);
    }

    #[test]
    fn the_real_house_is_the_house_the_plan_asked_for() {
        let def = house();
        assert_eq!(def.id, "house-v1");
        assert_eq!(def.chunks.len(), 4, "2x2 chunks, 32x32 tiles");
        assert_eq!(def.rooms.len(), 4);
        assert_eq!(def.palette.len(), 22, "every tile of the casa-v1 atlas");
        assert_eq!(def.slots.len(), def.objects.len());
        assert!(def.slots.len() >= 12);
        for kind in REQUIRED_KINDS {
            assert!(def.objects.iter().any(|o| o.kind == kind), "no {kind}");
        }
        assert_eq!(
            def.objects
                .iter()
                .filter(|o| o.kind == "object/bed")
                .count(),
            2
        );
    }

    #[test]
    fn every_room_reaches_every_other_room() {
        let def = house();
        let mut scene = Scene::new(&def).unwrap();
        for r in &def.rooms {
            let c = r.centre();
            assert!(scene.grid().passable(c), "{} centre is blocked", r.id);
            assert!(scene.reachable(c), "{} is cut off from the spawn", r.id);
        }
    }

    #[test]
    fn the_spawn_is_not_silently_corrected() {
        let def = house();
        let scene = Scene::new(&def).unwrap();
        let spawn = def
            .markers
            .iter()
            .find(|m| m.name == "spawn")
            .expect("a spawn marker")
            .tile;
        assert!(scene.grid().passable(spawn));
        assert_eq!(
            scene.grid().nearest_passable(spawn, 4),
            Some(spawn),
            "the world would have moved this spawn"
        );
    }

    #[test]
    fn a_spawn_on_a_bed_is_caught() {
        let mut def = house();
        let bed = def
            .objects
            .iter()
            .find(|o| o.kind == "object/bed")
            .unwrap()
            .tile;
        for m in &mut def.markers {
            if m.name == "spawn" {
                m.tile = bed;
            }
        }
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_MARKER_BLOCKED"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_room_sealed_off_is_caught() {
        let mut def = house();
        // Fill the two doorway tiles between the bedroom and the living room,
        // and the two between the bedroom and the study, with plaster.
        let plaster = def
            .palette
            .iter()
            .position(|t| t.key == "wall/plaster")
            .unwrap() as u8;
        let chunk = |def: &mut MapDef, x: usize, y: usize, v: u8| {
            let ci = (y / 16) * 2 + (x / 16);
            let i = (y % 16) * 16 + (x % 16);
            def.chunks[ci].wall[i] = v;
            def.chunks[ci].height[i] = 32;
        };
        for (x, y) in [(6, 15), (7, 15), (15, 7), (15, 8)] {
            chunk(&mut def, x, y, plaster);
        }
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_UNREACHABLE"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_blocked_approach_is_caught() {
        let mut def = house();
        let bench = def
            .objects
            .iter()
            .find(|o| o.kind == "object/workbench")
            .unwrap()
            .clone();
        def.slots.push(SlotDef {
            id: "test-blocker".into(),
            kind: SlotKind::Floor,
            tile: Tile::new(bench.tile.x, bench.tile.y + 1),
            accepts: vec!["object/chest".into()],
            default: Some("object/chest".into()),
            dir: 0,
        });
        def.objects.push(ObjectDef {
            id: 900,
            kind: "object/chest".into(),
            tile: Tile::new(bench.tile.x, bench.tile.y + 1),
            dir: 0,
            slot: Some("test-blocker".into()),
        });
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_APPROACH_BLOCKED"),
            "{issues:#?}"
        );
    }

    #[test]
    fn an_object_in_a_wall_is_caught() {
        let mut def = house();
        let slot = def
            .slots
            .iter_mut()
            .find(|s| s.kind == SlotKind::Floor)
            .unwrap();
        slot.tile = Tile::new(0, 0);
        let id = slot.id.clone();
        for o in &mut def.objects {
            if o.slot.as_deref() == Some(id.as_str()) {
                o.tile = Tile::new(0, 0);
            }
        }
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_OBJECT_IN_WALL"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_palette_that_lies_about_the_art_is_caught() {
        let mut def = house();
        let bed = def
            .palette
            .iter_mut()
            .find(|t| t.key == "object/bed")
            .unwrap();
        bed.walkable = true;
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_TILE_MISMATCH"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_tile_the_atlas_does_not_have_is_caught() {
        let mut def = house();
        def.palette.push(TileDef {
            key: "object/hovercraft".into(),
            height: 10,
            occludes: false,
            footprint: [1, 1],
            walkable: false,
        });
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_TILE_UNKNOWN"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_house_without_a_bed_is_caught() {
        let mut def = house();
        let beds: Vec<String> = def
            .objects
            .iter()
            .filter(|o| o.kind == "object/bed")
            .filter_map(|o| o.slot.clone())
            .collect();
        def.objects.retain(|o| o.kind != "object/bed");
        def.slots.retain(|s| !beds.contains(&s.id));
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_MISSING_KIND"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_slot_that_rejects_its_own_object_is_caught() {
        let mut def = house();
        def.slots[0].accepts = vec!["object/plant".into()];
        def.slots[0].default = Some("object/plant".into());
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_SLOT_REJECTS_OBJECT"),
            "{issues:#?}"
        );
    }

    #[test]
    fn an_empty_slot_is_caught() {
        let mut def = house();
        let id = def.slots[0].id.clone();
        def.objects
            .retain(|o| o.slot.as_deref() != Some(id.as_str()));
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_SLOT_EMPTY"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_table_slot_floating_in_the_air_is_caught() {
        let mut def = house();
        let (i, tile) = def
            .slots
            .iter()
            .enumerate()
            .find(|(_, s)| s.kind == SlotKind::Table)
            .map(|(i, s)| (i, s.tile))
            .unwrap();
        let moved = Tile::new(tile.x, tile.y + 4);
        def.slots[i].tile = moved;
        let id = def.slots[i].id.clone();
        for o in &mut def.objects {
            if o.slot.as_deref() == Some(id.as_str()) {
                o.tile = moved;
            }
        }
        let issues = check(&def, &atlas(), &[]);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_TABLE_SLOT_NO_SURFACE"),
            "{issues:#?}"
        );
    }

    #[test]
    fn overlapping_rooms_and_a_missing_marker_are_caught() {
        let mut def = house();
        def.rooms.push(RoomDef {
            id: "ghost".into(),
            rect: def.rooms[0].rect,
        });
        def.markers.retain(|m| m.name != "front-door");
        def.markers.push(MarkerDef {
            name: "spawn".into(),
            tile: def.markers[0].tile,
        });
        let issues = check(&def, &atlas(), &[]);
        let c = codes(&issues);
        assert!(c.contains(&"ERR_HOUSE_ROOM_OVERLAP"), "{issues:#?}");
        assert!(c.contains(&"ERR_HOUSE_MARKER_MISSING"), "{issues:#?}");
        assert!(c.contains(&"ERR_HOUSE_MARKER_DUPLICATE"), "{issues:#?}");
    }

    #[test]
    fn clock_strings_round_trip() {
        assert_eq!(parse_hm("00:00"), Some(0));
        assert_eq!(parse_hm("07:30"), Some(450));
        assert_eq!(parse_hm("23:59"), Some(1439));
        assert_eq!(parse_hm("7:30"), None, "HH:MM only");
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("07:60"), None);
        assert_eq!(parse_hm("morning"), None);
        assert_eq!(format_hm(450), "07:30");
        assert_eq!(format_hm(0), "00:00");
    }

    #[test]
    fn a_routine_file_becomes_the_simulation_type() {
        let def: RoutineDef = serde_json::from_str(COORDINATOR).unwrap();
        let r = routine_of(&def).unwrap();
        assert_eq!(r.len(), def.entries.len());
        let mins: Vec<u16> = r.entries().iter().map(|e| e.minute).collect();
        let mut sorted = mins.clone();
        sorted.sort_unstable();
        assert_eq!(mins, sorted, "Routine::new sorts by minute");
        assert_eq!(
            r.current(parse_hm("10:00").unwrap()),
            Some(&Decision::Work(ObjectId(9)))
        );
        assert_eq!(
            r.current(parse_hm("03:00").unwrap()),
            Some(&Decision::Sleep)
        );
    }

    #[test]
    fn every_role_of_the_roster_has_a_routine() {
        let roles: BTreeSet<String> = routines().iter().map(|(_, r)| r.role.clone()).collect();
        for want in ["coordinator", "worker", "advisor"] {
            assert!(roles.contains(want), "no routine for {want}");
        }
    }

    #[test]
    fn a_routine_walking_into_a_wall_is_caught() {
        let mut rs = routines();
        rs[0].1.entries.push(RoutineEntryDef {
            at: "03:15".into(),
            act: RoutineAct::Goto { tile: [0, 0] },
        });
        let issues = check(&house(), &atlas(), &rs);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_ROUTINE_TILE"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_routine_using_the_wrong_object_is_caught() {
        let mut rs = routines();
        let plant = house()
            .objects
            .iter()
            .find(|o| o.kind == "object/plant")
            .unwrap()
            .id;
        rs[0].1.entries.push(RoutineEntryDef {
            at: "03:20".into(),
            act: RoutineAct::Work { object: plant },
        });
        rs[0].1.entries.push(RoutineEntryDef {
            at: "03:25".into(),
            act: RoutineAct::Sit { object: 4242 },
        });
        let issues = check(&house(), &atlas(), &rs);
        let c = codes(&issues);
        assert!(c.contains(&"ERR_HOUSE_ROUTINE_OBJECT_KIND"), "{issues:#?}");
        assert!(c.contains(&"ERR_HOUSE_ROUTINE_OBJECT"), "{issues:#?}");
    }

    #[test]
    fn a_routine_for_another_map_or_version_is_caught() {
        let mut rs = routines();
        rs[0].1.map = "house-v2".into();
        rs[1].1.version = 99;
        rs[2].1.entries.retain(|e| e.act != RoutineAct::Sleep);
        rs[3].1.entries.push(RoutineEntryDef {
            at: "9 am".into(),
            act: RoutineAct::Idle,
        });
        let issues = check(&house(), &atlas(), &rs);
        let c = codes(&issues);
        assert!(c.contains(&"ERR_HOUSE_ROUTINE_MAP"), "{issues:#?}");
        assert!(c.contains(&"ERR_HOUSE_ROUTINE_VERSION"), "{issues:#?}");
        assert!(c.contains(&"ERR_HOUSE_ROUTINE_NO_SLEEP"), "{issues:#?}");
        assert!(c.contains(&"ERR_HOUSE_ROUTINE_TIME"), "{issues:#?}");
    }

    #[test]
    fn a_long_say_is_caught() {
        let mut rs = routines();
        rs[0].1.entries.push(RoutineEntryDef {
            at: "03:30".into(),
            act: RoutineAct::Say {
                text: "a".repeat(MAX_SAY_BYTES + 1),
            },
        });
        let issues = check(&house(), &atlas(), &rs);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_ROUTINE_SAY"),
            "{issues:#?}"
        );
    }

    #[test]
    fn two_entries_at_the_same_minute_are_caught() {
        let mut rs = routines();
        let first = rs[0].1.entries[0].at.clone();
        rs[0].1.entries.push(RoutineEntryDef {
            at: first,
            act: RoutineAct::Idle,
        });
        let issues = check(&house(), &atlas(), &rs);
        assert!(
            codes(&issues).contains(&"ERR_HOUSE_ROUTINE_COLLISION"),
            "{issues:#?}"
        );
    }

    #[test]
    fn a_map_the_world_refuses_stops_the_run() {
        let mut def = house();
        def.version = 9;
        let issues = check(&def, &atlas(), &routines());
        assert_eq!(codes(&issues), vec!["ERR_HOUSE_MAP_INVALID"]);
    }

    #[test]
    fn issues_read_as_a_line_of_report() {
        let i = Issue::new("ERR_HOUSE_MARKER_MISSING", "there is no spawn marker");
        assert_eq!(
            i.to_string(),
            "ERR_HOUSE_MARKER_MISSING: there is no spawn marker"
        );
    }
}
