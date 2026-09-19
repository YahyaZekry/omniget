//! Pack a synthetic source tree, then check the atlas it produced.
//!
//! No art is committed: every fixture is generated in a temp dir at test time.

use std::path::Path;

use omniget_world_tools::check::check_file;
use omniget_world_tools::fixture::{write_character, write_manifest, write_tiles, TempDir};
use omniget_world_tools::pack::{pack, PackError, PackOptions};
use omniget_world_tools::png;
use omniget_world_tools::schema::{Atlas, DirEntry};

fn read_atlas(path: &Path) -> Atlas {
    let text = std::fs::read_to_string(path).expect("read atlas.json");
    serde_json::from_str(&text).expect("parse atlas.json")
}

#[test]
fn packs_a_character_and_the_atlas_checks_clean() {
    let dir = TempDir::new("e2e-char").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("idle", 4), ("walk", 8)], 48, 64).expect("fixture");
    let out = dir.path().join("out");

    let report = pack(&[src], &out, &PackOptions::default()).expect("pack");
    assert_eq!(
        report.frame_count,
        (4 + 8) * 5,
        "5 unique directions per animation"
    );
    assert_eq!(report.anim_count, 2);
    assert_eq!(report.page_paths.len(), 1);

    let atlas = read_atlas(&report.atlas_path);
    assert_eq!(atlas.version, 1);
    assert_eq!(atlas.pages, vec!["omni-0.png".to_string()]);
    assert_eq!(atlas.padding, Some(2));
    assert_eq!(atlas.extrude, Some(1));

    // The three mirrored directions carry no art.
    let walk = atlas.anims.get("omni/walk").expect("walk");
    assert_eq!(walk.fps, 10);
    for (dir_name, expected) in [("SE", "SW"), ("E", "W"), ("NE", "NW")] {
        let dirs = walk.dirs.iter();
        let entry = dirs
            .into_iter()
            .find(|(name, _)| *name == dir_name)
            .map(|(_, e)| e)
            .expect("direction");
        match entry {
            DirEntry::Mirror(m) => assert_eq!(m.mirror_of, expected),
            DirEntry::Frames(_) => panic!("{dir_name} must be a mirror"),
        }
    }
    assert!(atlas.frames.contains_key("omni/walk/S/7"));
    assert!(
        !atlas.frames.keys().any(|k| k.contains("/E/")),
        "no sheet for mirrored directions"
    );

    // Default pivot is the bottom centre of a 48x64 frame.
    assert_eq!(atlas.frames["omni/walk/S/0"].pivot, [24, 63]);

    let issues = check_file(&report.atlas_path);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn the_page_is_srgb_with_no_embedded_profile_and_within_the_ceiling() {
    let dir = TempDir::new("e2e-page").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("idle", 2)], 32, 32).expect("fixture");
    let out = dir.path().join("out");
    let report = pack(&[src], &out, &PackOptions::default()).expect("pack");

    let bytes = std::fs::read(&report.page_paths[0]).expect("read page");
    let info = png::inspect(&bytes).expect("inspect");
    assert!(
        info.color_profile_chunks().is_empty(),
        "chunks: {:?}",
        info.chunks
    );
    assert!(info.width <= 2048 && info.height <= 2048);
    let atlas = read_atlas(&report.atlas_path);
    assert_eq!(
        atlas.page_sizes[0],
        [info.width, info.height],
        "page_sizes matches the file"
    );
}

#[test]
fn packing_twice_gives_byte_identical_output() {
    let dir = TempDir::new("e2e-det").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("idle", 3), ("walk", 6)], 40, 56).expect("fixture");

    let sources = std::slice::from_ref(&src);
    let a = pack(sources, &dir.path().join("a"), &PackOptions::default()).expect("pack a");
    let b = pack(sources, &dir.path().join("b"), &PackOptions::default()).expect("pack b");

    assert_eq!(
        std::fs::read(&a.page_paths[0]).expect("a"),
        std::fs::read(&b.page_paths[0]).expect("b"),
        "the PNG page must be byte for byte the same"
    );
    assert_eq!(
        std::fs::read_to_string(&a.atlas_path).expect("a"),
        std::fs::read_to_string(&b.atlas_path).expect("b"),
    );
}

#[test]
fn the_manifest_sets_fps_and_pivot() {
    let dir = TempDir::new("e2e-manifest").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("walk", 2), ("sit", 2)], 48, 64).expect("fixture");
    write_manifest(
        &src,
        r#"{ "default_fps": 8, "anims": { "omni/walk": { "fps": 12, "pivot": [24, 60], "z_base": 4, "loop": false } } }"#,
    )
    .expect("manifest");

    let report = pack(&[src], &dir.path().join("out"), &PackOptions::default()).expect("pack");
    let atlas = read_atlas(&report.atlas_path);
    assert_eq!(atlas.anims["omni/walk"].fps, 12);
    assert!(!atlas.anims["omni/walk"].looping);
    assert_eq!(
        atlas.anims["omni/sit"].fps, 8,
        "default_fps applies to the rest"
    );
    assert_eq!(atlas.frames["omni/walk/N/1"].pivot, [24, 60]);
    assert_eq!(atlas.frames["omni/walk/N/1"].z_base, 4);
    assert!(check_file(&report.atlas_path).is_empty());
}

#[test]
fn tiles_carry_height_occlusion_and_footprint() {
    let dir = TempDir::new("e2e-tiles").expect("temp dir");
    let src = dir.path().join("casa-v1-src");
    write_tiles(&src).expect("fixture");

    let report = pack(&[src], &dir.path().join("out"), &PackOptions::default()).expect("pack");
    assert_eq!(report.tile_count, 3);
    let atlas = read_atlas(&report.atlas_path);
    assert_eq!(
        atlas.pages,
        vec!["casa-v1-0.png".to_string()],
        "the -src suffix is dropped"
    );
    let wall = &atlas.tiles["wall/brick"];
    assert_eq!(wall.frame, "casa-v1/wall/brick");
    assert_eq!(wall.height, 32);
    assert!(wall.occludes && !wall.walkable);
    assert_eq!(atlas.tiles["floor/wood"].height, 0);
    assert_eq!(atlas.tiles["object/desk"].footprint, [2, 1]);
    assert!(check_file(&report.atlas_path).is_empty());
}

#[test]
fn a_character_and_a_tile_source_pack_into_one_atlas() {
    let dir = TempDir::new("e2e-both").expect("temp dir");
    let omni = dir.path().join("omni-src");
    let casa = dir.path().join("casa-v1-src");
    write_character(&omni, "omni", &[("idle", 2)], 32, 48).expect("fixture");
    write_tiles(&casa).expect("fixture");

    let report = pack(
        &[omni, casa],
        &dir.path().join("out"),
        &PackOptions::default(),
    )
    .expect("pack");
    assert_eq!(report.frame_count, 2 * 5 + 3);
    let atlas = read_atlas(&report.atlas_path);
    assert_eq!(
        atlas.pages,
        vec!["omni-0.png".to_string()],
        "the prefix is the first sheet"
    );
    assert!(atlas.frames.contains_key("casa-v1/floor/wood"));
    assert!(check_file(&report.atlas_path).is_empty());
}

#[test]
fn a_small_page_ceiling_spills_into_several_pages() {
    let dir = TempDir::new("e2e-pages").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("walk", 6)], 48, 48).expect("fixture");
    let opts = PackOptions {
        page_max: 128,
        ..PackOptions::default()
    };

    let report = pack(&[src], &dir.path().join("out"), &opts).expect("pack");
    assert!(
        report.page_paths.len() > 1,
        "30 frames of 48px cannot fit one 128px page"
    );
    let atlas = read_atlas(&report.atlas_path);
    assert_eq!(atlas.pages.len(), report.page_paths.len());
    assert_eq!(atlas.page_sizes.len(), atlas.pages.len());
    assert!(atlas.frames.values().any(|f| f.page > 0));
    let issues = check_file(&report.atlas_path);
    assert!(issues.is_empty(), "{issues:?}");
}

#[test]
fn the_extrusion_border_copies_the_frame_edge() {
    let dir = TempDir::new("e2e-extrude").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("idle", 1)], 16, 16).expect("fixture");
    let report = pack(&[src], &dir.path().join("out"), &PackOptions::default()).expect("pack");

    let page = image::open(&report.page_paths[0])
        .expect("open page")
        .to_rgba8();
    let atlas = read_atlas(&report.atlas_path);
    let f = atlas.frames["omni/idle/S/0"];
    let inside = *page.get_pixel(f.x, f.y);
    assert_eq!(*page.get_pixel(f.x - 1, f.y), inside, "left extrusion");
    assert_eq!(*page.get_pixel(f.x, f.y - 1), inside, "top extrusion");
    assert_eq!(*page.get_pixel(f.x - 1, f.y - 1), inside, "corner");
}

#[test]
fn a_mirrored_direction_in_the_source_is_refused() {
    let dir = TempDir::new("e2e-mirror").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("walk", 1)], 16, 16).expect("fixture");
    // Someone drew E by hand: that is exactly what the format forbids.
    let e_dir = src.join("omni/walk/E");
    std::fs::create_dir_all(&e_dir).expect("mkdir");
    std::fs::copy(src.join("omni/walk/W/0.png"), e_dir.join("0.png")).expect("copy");

    let err =
        pack(&[src], &dir.path().join("out"), &PackOptions::default()).expect_err("must fail");
    assert_eq!(err.code(), "ERR_ATLAS_MIRROR_SHEET");
    assert!(matches!(err, PackError::MirrorSheet { .. }));
}

#[test]
fn a_missing_unique_direction_is_refused() {
    let dir = TempDir::new("e2e-missing").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("walk", 1)], 16, 16).expect("fixture");
    std::fs::remove_dir_all(src.join("omni/walk/NW")).expect("remove NW");

    let err =
        pack(&[src], &dir.path().join("out"), &PackOptions::default()).expect_err("must fail");
    assert_eq!(err.code(), "ERR_ATLAS_MISSING_DIR");
}

#[test]
fn a_frame_file_that_is_not_numbered_is_refused() {
    let dir = TempDir::new("e2e-name").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("walk", 1)], 16, 16).expect("fixture");
    std::fs::copy(
        src.join("omni/walk/S/0.png"),
        src.join("omni/walk/S/first.png"),
    )
    .expect("copy");

    let err =
        pack(&[src], &dir.path().join("out"), &PackOptions::default()).expect_err("must fail");
    assert_eq!(err.code(), "ERR_ATLAS_SRC_FRAME_NAME");
}

#[test]
fn check_catches_a_page_with_an_embedded_colour_profile() {
    let dir = TempDir::new("e2e-profile").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("idle", 1)], 16, 16).expect("fixture");
    let out = dir.path().join("out");
    let report = pack(&[src], &out, &PackOptions::default()).expect("pack");

    // Forge the page an image editor would have written.
    let bytes = std::fs::read(&report.page_paths[0]).expect("read");
    let forged =
        png::insert_chunk_after_ihdr(&bytes, b"gAMA", &45455u32.to_be_bytes()).expect("forge");
    std::fs::write(&report.page_paths[0], forged).expect("write");

    let issues = check_file(&report.atlas_path);
    assert_eq!(
        issues.iter().map(|i| i.code).collect::<Vec<_>>(),
        vec!["ERR_ATLAS_COLOR_PROFILE"]
    );
}

#[test]
fn check_catches_a_missing_page_file() {
    let dir = TempDir::new("e2e-nopage").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(&src, "omni", &[("idle", 1)], 16, 16).expect("fixture");
    let out = dir.path().join("out");
    let report = pack(&[src], &out, &PackOptions::default()).expect("pack");
    std::fs::remove_file(&report.page_paths[0]).expect("remove");

    let issues = check_file(&report.atlas_path);
    assert!(
        issues.iter().any(|i| i.code == "ERR_ATLAS_PAGE_MISSING"),
        "{issues:?}"
    );
}

#[test]
fn check_reports_a_broken_atlas_file_as_one_issue() {
    let dir = TempDir::new("e2e-json").expect("temp dir");
    let path = dir.path().join("atlas.json");
    std::fs::write(&path, "{ not json").expect("write");
    let issues = check_file(&path);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].code, "ERR_ATLAS_JSON");

    let missing = check_file(&dir.path().join("nope.json"));
    assert_eq!(missing[0].code, "ERR_ATLAS_IO");
}

#[test]
fn check_catches_a_page_over_the_2048_ceiling() {
    let dir = TempDir::new("e2e-big").expect("temp dir");
    let page = image::RgbaImage::from_pixel(2049, 8, image::Rgba([0, 0, 0, 0]));
    page.save_with_format(dir.path().join("big-0.png"), image::ImageFormat::Png)
        .expect("save");
    std::fs::write(
        dir.path().join("atlas.json"),
        r#"{ "version": 1, "pages": ["big-0.png"],
             "frames": { "big/a": { "page": 0, "x": 0, "y": 0, "w": 8, "h": 8, "pivot": [4, 7] } } }"#,
    )
    .expect("write");

    let issues = check_file(&dir.path().join("atlas.json"));
    assert_eq!(
        issues.iter().map(|i| i.code).collect::<Vec<_>>(),
        vec!["ERR_ATLAS_PAGE_TOO_BIG"]
    );
}

#[test]
fn packed_frames_never_overlap_on_a_page() {
    let dir = TempDir::new("e2e-overlap").expect("temp dir");
    let src = dir.path().join("omni-src");
    write_character(
        &src,
        "omni",
        &[("idle", 5), ("walk", 7), ("sit", 3)],
        24,
        40,
    )
    .expect("fixture");
    let report = pack(&[src], &dir.path().join("out"), &PackOptions::default()).expect("pack");
    let atlas = read_atlas(&report.atlas_path);

    let frames: Vec<_> = atlas.frames.values().collect();
    for (i, a) in frames.iter().enumerate() {
        for b in frames.iter().skip(i + 1) {
            if a.page != b.page {
                continue;
            }
            let disjoint =
                a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
            assert!(disjoint, "frames overlap: {a:?} vs {b:?}");
        }
    }
    assert!(check_file(&report.atlas_path).is_empty());
}
