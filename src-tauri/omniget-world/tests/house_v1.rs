//! The real house, run by the real simulation.
//!
//! `tests/determinism.rs` runs the minimal example of the format and freezes a
//! fingerprint over it. This file runs the map the app actually ships,
//! `static/world/house-v1.json`, and asserts that it loads, that a day of
//! routines plays out in it, and that nothing the content author writes by
//! hand is silently corrected on the way in. It is `include_str!`d from the
//! repository rather than copied into `tests/fixtures/`, so there is one
//! house, not two that drift apart. The content itself is validated in depth
//! by `house-check` in `omniget-world-tools`.

use omniget_world::ents::EntId;
use omniget_world::map::Tile;
use omniget_world::sim::{hm, Decision, Routine, RoutineEntry};
use omniget_world::{Interest, MapDef, ObjectId, World};

const HOUSE: &str = include_str!("../../../static/world/house-v1.json");

fn house() -> MapDef {
    serde_json::from_str(HOUSE).expect("house-v1.json parses as a MapDef")
}

#[test]
fn the_shipped_house_loads() {
    let w = World::new(11, house()).expect("house-v1 loads");
    let map = w.map();
    assert_eq!(map.id, "house-v1");
    assert_eq!((map.w, map.h), (32, 32), "2x2 chunks");
    assert_eq!(map.rooms.len(), 4);
    assert_eq!(w.object_count(), 41);
    assert!(map.marker("spawn").is_some());
    assert!(map.marker("front-door").is_some());
}

#[test]
fn the_spawn_marker_is_taken_as_written() {
    let mut w = World::new(11, house()).unwrap();
    let spawn = w.map().marker("spawn").unwrap();
    w.step(&[omniget_world::Input::Spawn {
        ent: EntId(1),
        name: "Omni".into(),
        at: spawn,
    }]);
    assert_eq!(
        w.tile_of(EntId(1)),
        Some(spawn),
        "the world moved the spawn, so the marker is on furniture"
    );
}

#[test]
fn an_agent_walks_from_the_door_to_the_workbench() {
    let mut w = World::new(11, house()).unwrap();
    let door = w.map().marker("front-door").unwrap();
    let bench = w.map().marker("workbench").unwrap();
    w.step(&[omniget_world::Input::Spawn {
        ent: EntId(1),
        name: "Ada".into(),
        at: door,
    }]);
    w.step(&[omniget_world::Input::Move {
        ent: EntId(1),
        to: bench,
    }]);
    // 32x32 tiles at 38/256 of a tile per tick is at most ~450 ticks corner to
    // corner; 900 is slack, not a measurement.
    for _ in 0..900 {
        w.step(&[]);
        if w.tile_of(EntId(1)) == Some(bench) {
            return;
        }
    }
    panic!(
        "never arrived at the workbench, got {:?}",
        w.tile_of(EntId(1))
    );
}

#[test]
fn a_day_of_the_coordinator_routine_runs() {
    let mut w = World::new(11, house()).unwrap();
    let spawn = w.map().marker("spawn").unwrap();
    w.step(&[omniget_world::Input::Spawn {
        ent: EntId(1),
        name: "Omni".into(),
        at: spawn,
    }]);
    // The same entries as static/world/routines/coordinator.json, by hand:
    // this crate does not read that file, the app does.
    w.step(&[omniget_world::Input::SetRoutine {
        ent: EntId(1),
        routine: Routine::new(vec![
            RoutineEntry {
                minute: hm(9, 0),
                decision: Decision::Work(ObjectId(9)),
            },
            RoutineEntry {
                minute: hm(17, 30),
                decision: Decision::Sit(ObjectId(18)),
            },
            RoutineEntry {
                minute: hm(22, 0),
                decision: Decision::Sleep,
            },
        ]),
    }]);
    // Skip to 09:30 and check the routine put the agent at the bench.
    w.catch_up(9 * 3_600_000 + 1_800_000);
    assert_eq!(
        w.activity_of(EntId(1)),
        Some(omniget_world::ents::Activity::Working(ObjectId(9)))
    );
    w.catch_up(13 * 3_600_000);
    assert_eq!(
        w.activity_of(EntId(1)),
        Some(omniget_world::ents::Activity::Sleeping)
    );
}

#[test]
fn a_snapshot_of_the_house_stays_small() {
    let mut w = World::new(11, house()).unwrap();
    for i in 1..=8u32 {
        w.step(&[omniget_world::Input::Spawn {
            ent: EntId(i),
            name: format!("a{i}"),
            at: w.map().marker("spawn").unwrap(),
        }]);
    }
    let bytes = w.snapshot().encode();
    // 29 objects plus 8 agents. The budget in the plan is about the diff, not
    // the snapshot; this is a regression guard on the content, not a target.
    assert!(bytes.len() < 2048, "snapshot is {} bytes", bytes.len());

    let before = w.tick();
    w.step(&[]);
    let diff = w.diff_since(before, &Interest::ALL).unwrap();
    assert!(diff.encode().len() < 512);
}

#[test]
fn the_replica_of_the_house_matches_the_authority() {
    let mut w = World::new(11, house()).unwrap();
    let mut replica = World::new(11, house()).unwrap();
    assert_eq!(replica.snapshot(), w.snapshot());
    let spawn = w.map().marker("spawn").unwrap();
    let script: [(u64, Vec<omniget_world::Input>); 2] = [
        (
            0,
            vec![omniget_world::Input::Spawn {
                ent: EntId(1),
                name: "Omni".into(),
                at: spawn,
            }],
        ),
        (
            1,
            vec![omniget_world::Input::Move {
                ent: EntId(1),
                to: Tile::new(23, 22),
            }],
        ),
    ];
    for t in 0..60u64 {
        let inputs: Vec<omniget_world::Input> = script
            .iter()
            .filter(|(at, _)| *at == t)
            .flat_map(|(_, i)| i.clone())
            .collect();
        let from = w.tick();
        w.step(&inputs);
        let diff = w.diff_since(from, &Interest::ALL).unwrap();
        replica.apply(&diff).unwrap();
    }
    assert_eq!(replica.snapshot().encode(), w.snapshot().encode());
}
