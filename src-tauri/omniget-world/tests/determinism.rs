//! The promises the rest of the plan is built on, tested end to end against
//! the real map fixture: same seed and same inputs give the same world on
//! macOS, Linux and Windows; a replica that applies diffs is identical to the
//! authority; and interest filtering removes what it says it removes.
//!
//! The map is `tests/fixtures/house-v1.min.json`, the minimal example of the
//! `house-v1` format. It is parsed here with `serde_json`, which is a
//! dev-dependency: the crate itself never parses JSON.

use omniget_world::ents::EntId;
use omniget_world::map::Tile;
use omniget_world::sim::{hm, Decision, Routine, RoutineEntry, SleepState};
use omniget_world::snapshot::binary::{FORMAT_VERSION, MAGIC};
use omniget_world::{Diff, Input, Interest, MapDef, ObjectId, Snapshot, World, WorldEvent};

const FIXTURE: &str = include_str!("fixtures/house-v1.min.json");

fn map() -> MapDef {
    serde_json::from_str(FIXTURE).expect("house-v1.min.json parses")
}

/// A fixed script: enough kinds of input that a change in any subsystem moves
/// the fingerprint.
fn script() -> Vec<(u64, Vec<Input>)> {
    vec![
        (
            0,
            vec![
                Input::Spawn {
                    ent: EntId(1),
                    name: "Omni".into(),
                    at: Tile::new(8, 12),
                },
                Input::Spawn {
                    ent: EntId(2),
                    name: "Ada".into(),
                    at: Tile::new(2, 5),
                },
                Input::Spawn {
                    ent: EntId(3),
                    name: "Bo".into(),
                    at: Tile::new(12, 3),
                },
            ],
        ),
        (
            1,
            vec![Input::SetRoutine {
                ent: EntId(1),
                routine: Routine::new(vec![
                    RoutineEntry {
                        minute: hm(0, 3),
                        decision: Decision::Work(ObjectId(6)),
                    },
                    RoutineEntry {
                        minute: hm(0, 30),
                        decision: Decision::Sleep,
                    },
                ]),
            }],
        ),
        (
            2,
            vec![Input::Move {
                ent: EntId(2),
                to: Tile::new(13, 12),
            }],
        ),
        (
            3,
            vec![Input::Decision {
                ent: EntId(3),
                decision: Decision::Sit(ObjectId(7)),
            }],
        ),
        (
            40,
            vec![Input::SetEnergy {
                ent: EntId(2),
                energy: 30,
            }],
        ),
        (
            60,
            vec![Input::Decision {
                ent: EntId(1),
                decision: Decision::Say("olá, casa".into()),
            }],
        ),
        (
            80,
            vec![Input::PlaceObject {
                object: ObjectId(20),
                kind: "object/chair".into(),
                tile: Tile::new(10, 11),
                dir: 0,
                slot: None,
            }],
        ),
        (
            100,
            vec![Input::Interact {
                ent: EntId(1),
                object: ObjectId(4),
            }],
        ),
        (
            120,
            vec![Input::RemoveObject {
                object: ObjectId(5),
            }],
        ),
        (140, vec![Input::Despawn { ent: EntId(3) }]),
    ]
}

fn run(ticks: u64) -> World {
    let mut w = World::new(0x0417_2026_0918, map()).expect("map loads");
    let plan = script();
    for t in 0..ticks {
        let inputs: Vec<Input> = plan
            .iter()
            .filter(|(at, _)| *at == t)
            .flat_map(|(_, i)| i.clone())
            .collect();
        w.step(&inputs);
    }
    w
}

#[test]
fn the_fixture_is_a_valid_house() {
    let w = World::new(1, map()).expect("house-v1.min.json loads");
    assert_eq!(w.map().id, "house-v1");
    assert_eq!(w.map().atlas, "world/tiles/casa-v1/atlas.json");
    assert_eq!((w.map().w, w.map().h), (16, 16));
    assert_eq!(w.object_count(), 9, "every slot starts filled");
    assert_eq!(w.map().slots.len(), 9);
    assert!(w.map().marker("spawn").is_some());
    assert!(w.map().room_at(Tile::new(2, 3)).is_some());
    // The tile keys are the ones casa-v1's atlas actually ships.
    for t in &w.map().palette {
        assert!(
            t.key.starts_with("floor/")
                || t.key.starts_with("wall/")
                || t.key.starts_with("object/"),
            "{}",
            t.key
        );
    }
}

#[test]
fn the_same_seed_and_inputs_give_the_same_world() {
    let a = run(200);
    let b = run(200);
    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(a.snapshot().fingerprint(), b.snapshot().fingerprint());
}

#[test]
fn the_fingerprint_is_frozen_across_platforms() {
    // Recorded once and asserted forever. If this changes, either the
    // simulation changed on purpose — and the number is updated in the same
    // commit — or determinism broke. Same value on macOS, Linux and Windows:
    // no float, no HashMap, no pointer, no clock in the state.
    let w = run(200);
    assert_eq!(w.tick(), 200);
    assert_eq!(w.snapshot().fingerprint(), FROZEN_FINGERPRINT);
}

/// Recorded on macOS aarch64, 2026-09-18, with
/// `cargo test -p omniget-world --test determinism`. Re-recorded the same day
/// when agents started to hurry to a workstation (`WORK_HURRY`).
const FROZEN_FINGERPRINT: u64 = 11_527_404_577_070_949_077;

#[test]
fn a_replica_that_applies_diffs_equals_the_authority() {
    let mut authority = World::new(99, map()).expect("map");
    let mut replica = World::new(99, map()).expect("map");
    assert_eq!(authority.snapshot(), replica.snapshot());

    let plan = script();
    for t in 0..300u64 {
        let inputs: Vec<Input> = plan
            .iter()
            .filter(|(at, _)| *at == t)
            .flat_map(|(_, i)| i.clone())
            .collect();
        let before = authority.tick();
        authority.step(&inputs);
        let diff = authority
            .diff_since(before, &Interest::ALL)
            .expect("one tick back is always in history");
        let bytes = diff.encode();
        let decoded = Diff::decode(&bytes).expect("round trip");
        assert_eq!(decoded, diff);
        replica.apply(&decoded).expect("apply");
        assert_eq!(
            replica.snapshot(),
            authority.snapshot(),
            "diverged at tick {}",
            authority.tick()
        );
    }
}

#[test]
fn a_diff_over_many_ticks_lands_in_the_same_place() {
    let mut authority = World::new(5, map()).expect("map");
    let mut replica = World::new(5, map()).expect("map");
    authority.step(&[Input::Spawn {
        ent: EntId(1),
        name: "Omni".into(),
        at: Tile::new(8, 12),
    }]);
    let diff = authority.diff_since(0, &Interest::ALL).expect("history");
    replica.apply(&diff).expect("apply");
    authority.step(&[Input::Move {
        ent: EntId(1),
        to: Tile::new(2, 3),
    }]);
    let from = authority.tick() - 1;
    for _ in 0..40 {
        authority.step(&[]);
    }
    let diff = authority.diff_since(from, &Interest::ALL).expect("history");
    assert_eq!(diff.from, from);
    assert_eq!(diff.to, authority.tick());
    replica.apply(&diff).expect("apply");
    assert_eq!(replica.snapshot(), authority.snapshot());
}

#[test]
fn a_diff_older_than_the_history_is_refused() {
    let mut w = World::new(1, map()).expect("map");
    for _ in 0..200 {
        w.step(&[]);
    }
    let err = w.diff_since(0, &Interest::ALL).unwrap_err();
    assert_eq!(err.code(), "ERR_WORLD_TICK_TOO_OLD");
}

#[test]
fn a_diff_from_the_wrong_tick_or_map_is_refused() {
    let mut a = World::new(1, map()).expect("map");
    a.step(&[]);
    let diff = a.diff_since(0, &Interest::ALL).expect("history");
    let mut b = World::new(1, map()).expect("map");
    b.step(&[]);
    assert_eq!(b.apply(&diff).unwrap_err().code(), "ERR_WORLD_DIFF_GAP");

    let mut wrong = diff.clone();
    wrong.map_hash ^= 1;
    let mut c = World::new(1, map()).expect("map");
    assert_eq!(
        c.apply(&wrong).unwrap_err().code(),
        "ERR_WORLD_MAP_MISMATCH"
    );
}

#[test]
fn interest_filters_what_is_far_away() {
    let mut w = World::new(3, map()).expect("map");
    w.step(&[
        Input::Spawn {
            ent: EntId(1),
            name: "near".into(),
            at: Tile::new(8, 12),
        },
        Input::Spawn {
            ent: EntId(2),
            name: "far".into(),
            at: Tile::new(2, 2),
        },
    ]);
    let from = w.tick();
    w.step(&[Input::Move {
        ent: EntId(1),
        to: Tile::new(9, 12),
    }]);
    w.step(&[Input::Move {
        ent: EntId(2),
        to: Tile::new(3, 2),
    }]);

    let all = w.diff_since(from, &Interest::ALL).expect("history");
    let near = w
        .diff_since(from, &Interest::new(Tile::new(9, 12), 3))
        .expect("history");
    assert_eq!(all.ents.len(), 2);
    assert_eq!(near.ents.len(), 1);
    assert_eq!(near.ents[0].id, EntId(1));
    assert!(near.encode().len() < all.encode().len());
}

#[test]
fn an_agent_leaving_the_circle_is_reported_as_gone() {
    let mut w = World::new(3, map()).expect("map");
    w.step(&[Input::Spawn {
        ent: EntId(1),
        name: "walker".into(),
        at: Tile::new(8, 12),
    }]);
    let from = w.tick();
    w.step(&[Input::Move {
        ent: EntId(1),
        to: Tile::new(2, 3),
    }]);
    // Under the 64 ticks of history, and far enough for a radius of 2.
    for _ in 0..55 {
        w.step(&[]);
    }
    let diff = w
        .diff_since(from, &Interest::new(Tile::new(9, 12), 2))
        .expect("history");
    assert!(
        diff.ents.iter().any(|d| d.is_despawn() && d.id == EntId(1)),
        "the client must be told to drop it"
    );
}

#[test]
fn a_diff_with_eight_walking_agents_fits_the_budget() {
    let mut w = World::new(11, map()).expect("map");
    let spawns: Vec<Input> = (1..=8u32)
        .map(|i| Input::Spawn {
            ent: EntId(i),
            name: format!("agent-{i}"),
            at: Tile::new(7 + (i as i32 % 4), 8 + (i as i32 % 5)),
        })
        .collect();
    w.step(&spawns);
    let moves: Vec<Input> = (1..=8u32)
        .map(|i| Input::Move {
            ent: EntId(i),
            to: Tile::new(13, 13),
        })
        .collect();
    w.step(&moves);
    let mut worst = 0usize;
    for _ in 0..100 {
        let from = w.tick();
        w.step(&[]);
        let d = w.diff_since(from, &Interest::ALL).expect("history");
        worst = worst.max(d.encode().len());
    }
    assert!(worst < 2048, "worst diff was {worst} bytes, budget is 2 KB");
}

#[test]
fn a_snapshot_round_trips_through_its_bytes() {
    let w = run(150);
    let bytes = w.snapshot().encode();
    assert_eq!(&bytes[..4], &MAGIC);
    assert_eq!(bytes[5], FORMAT_VERSION);
    let back = Snapshot::decode(&bytes).expect("decode");
    assert_eq!(back, w.snapshot());
    assert_eq!(back.encode(), bytes, "encoding is canonical");
}

#[test]
fn catch_up_is_deterministic_and_skips_time() {
    let build = || {
        let mut w = World::new(77, map()).expect("map");
        w.step(&[Input::Spawn {
            ent: EntId(1),
            name: "Omni".into(),
            at: Tile::new(8, 12),
        }]);
        w.step(&[Input::SetRoutine {
            ent: EntId(1),
            routine: Routine::new(vec![
                RoutineEntry {
                    minute: hm(8, 0),
                    decision: Decision::Work(ObjectId(6)),
                },
                RoutineEntry {
                    minute: hm(22, 0),
                    decision: Decision::Sleep,
                },
            ]),
        }]);
        w
    };
    let mut a = build();
    let mut b = build();
    a.set_sleep(SleepState::Hibernating);
    b.set_sleep(SleepState::Hibernating);
    let ra = a.catch_up(8 * 3_600_000);
    let rb = b.catch_up(8 * 3_600_000);
    assert_eq!(ra, rb);
    assert_eq!(a.snapshot(), b.snapshot());
    assert_eq!(ra.caught_up, 288_000);
    assert_eq!(a.tick(), 2 + 288_000);
    assert!(a
        .last_events()
        .iter()
        .any(|e| matches!(e, WorldEvent::CaughtUp { ticks: 288_000 })));
}

#[test]
fn a_world_nobody_touches_produces_nothing() {
    let mut w = World::new(1, map()).expect("map");
    let from = w.tick();
    for _ in 0..50 {
        let r = w.step(&[]);
        assert_eq!(r.events, 0);
        assert_eq!(r.moved, 0);
    }
    let d = w.diff_since(from, &Interest::ALL).expect("history");
    assert!(d.is_empty());
    // Header, two ticks, the map hash, the RNG state, the sleep byte and three
    // zero counts. Nothing that scales with the world.
    assert!(d.encode().len() < 32, "{} bytes", d.encode().len());
}

#[test]
fn bad_inputs_are_refused_without_stopping_the_world() {
    let mut w = World::new(1, map()).expect("map");
    let r = w.step(&[
        Input::Move {
            ent: EntId(99),
            to: Tile::new(1, 1),
        },
        Input::Despawn { ent: EntId(99) },
        Input::Interact {
            ent: EntId(99),
            object: ObjectId(1),
        },
        Input::RemoveObject {
            object: ObjectId(999),
        },
        Input::PlaceObject {
            object: ObjectId(50),
            kind: "object/chair".into(),
            tile: Tile::new(900, 900),
            dir: 0,
            slot: None,
        },
        Input::Tick,
    ]);
    assert_eq!(r.rejected, 5);
    assert_eq!(r.accepted, 0);
    assert_eq!(w.tick(), 1);
    let codes: Vec<&str> = w
        .last_events()
        .iter()
        .filter_map(|e| match e {
            WorldEvent::Rejected { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(codes.len(), 5);
    assert!(codes.iter().all(|c| c.starts_with("ERR_WORLD_")));
}

#[test]
fn placing_an_object_blocks_the_path_through_it() {
    let mut w = World::new(1, map()).expect("map");
    w.step(&[Input::Spawn {
        ent: EntId(1),
        name: "Omni".into(),
        at: Tile::new(8, 12),
    }]);
    assert!(w.grid().passable(Tile::new(10, 11)));
    w.step(&[Input::PlaceObject {
        object: ObjectId(30),
        kind: "object/bookshelf".into(),
        tile: Tile::new(10, 11),
        dir: 0,
        slot: None,
    }]);
    assert!(!w.grid().passable(Tile::new(10, 11)));
    w.step(&[Input::RemoveObject {
        object: ObjectId(30),
    }]);
    assert!(w.grid().passable(Tile::new(10, 11)));
}

#[test]
fn a_slot_refuses_an_object_it_does_not_accept() {
    let mut w = World::new(1, map()).expect("map");
    let r = w.step(&[Input::PlaceObject {
        object: ObjectId(40),
        kind: "object/bed".into(),
        tile: Tile::new(8, 13),
        dir: 0,
        slot: Some("living-tv".into()),
    }]);
    assert_eq!(r.rejected, 1);
    let r = w.step(&[Input::PlaceObject {
        object: ObjectId(41),
        kind: "object/tv".into(),
        tile: Tile::new(8, 13),
        dir: 0,
        slot: Some("living-tv".into()),
    }]);
    assert_eq!(r.accepted, 1);
}
