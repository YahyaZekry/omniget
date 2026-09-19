//! Inputs into the world. Owned by f7-world-bridge.
//!
//! Three sources, one destination. The world's `Input` enum has no `serde`
//! (the crate keeps `serde` for the map JSON alone), so the JSON the route
//! sends is parsed by hand here, tagged on `type` in snake case like every
//! other payload in the app.
//!
//! 1. **The player**, through `world_input`: a click to walk, an object
//!    dragged onto a slot, a camera that moved (`interest`, which is not a
//!    world input at all — it is what the bridge filters diffs with).
//! 2. **The bus** (F2), through [`bus_input`]: diegetic work. A turn starting
//!    sends that agent to its desk; a tool call sends it to the workbench; a
//!    download queued sends the house agent to the shelf. This is the whole of
//!    "the house shows what the app is doing".
//! 3. **The quota**, through [`energy_inputs`]: energy is an *input* to the
//!    world (plan §9.1), never something the simulation invents. What is left
//!    of an account's window becomes that agent's energy, so a spent account
//!    visibly goes to bed.
//!
//! Every parse failure is `ERR_WORLD_BAD_INPUT: <what>`, which the route shows
//! verbatim; a bad decision from a model never reaches the tick at all.

use std::collections::BTreeMap;

use omniget_core::core::omni::bus::BusEvent;
use omniget_world::{Decision, EntId, Input, Interest, ObjectId, Routine, Tile};
use serde_json::Value;

/// What `world_input` accepted: something for the simulation, or a change to
/// what this client wants to hear about.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BridgeInput {
    World(Box<Input>),
    Interest(Interest),
}

fn bad(what: &str) -> String {
    format!("ERR_WORLD_BAD_INPUT: {what}")
}

fn field<'a>(v: &'a Value, name: &str) -> Result<&'a Value, String> {
    v.get(name).ok_or_else(|| bad(name))
}

fn u32_at(v: &Value, name: &str) -> Result<u32, String> {
    field(v, name)?
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| bad(name))
}

fn u8_at(v: &Value, name: &str) -> Result<u8, String> {
    field(v, name)?
        .as_u64()
        .and_then(|n| u8::try_from(n).ok())
        .ok_or_else(|| bad(name))
}

fn str_at(v: &Value, name: &str) -> Result<String, String> {
    field(v, name)?
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| bad(name))
}

/// `[x, y]` or `{ "x": .., "y": .. }`; both spellings show up in the route and
/// neither is worth an argument.
fn tile_at(v: &Value, name: &str) -> Result<Tile, String> {
    let t = field(v, name)?;
    if let Some(arr) = t.as_array() {
        if arr.len() == 2 {
            let x = arr[0].as_i64().ok_or_else(|| bad(name))?;
            let y = arr[1].as_i64().ok_or_else(|| bad(name))?;
            return Ok(Tile::new(
                i32::try_from(x).map_err(|_| bad(name))?,
                i32::try_from(y).map_err(|_| bad(name))?,
            ));
        }
        return Err(bad(name));
    }
    let x = t
        .get("x")
        .and_then(|n| n.as_i64())
        .ok_or_else(|| bad(name))?;
    let y = t
        .get("y")
        .and_then(|n| n.as_i64())
        .ok_or_else(|| bad(name))?;
    Ok(Tile::new(
        i32::try_from(x).map_err(|_| bad(name))?,
        i32::try_from(y).map_err(|_| bad(name))?,
    ))
}

/// One decision, in the shape the brain and the route both write.
pub fn parse_decision(v: &Value) -> Result<Decision, String> {
    let kind = v
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| bad("decision.type"))?;
    Ok(match kind {
        "go_to" => Decision::GoTo(tile_at(v, "to")?),
        "sit" => Decision::Sit(ObjectId(u32_at(v, "object")?)),
        "sleep" => Decision::Sleep,
        "work" => Decision::Work(ObjectId(u32_at(v, "object")?)),
        // The mailbox truncates to MAX_SAY_BYTES on a char boundary; we do not
        // second-guess it here.
        "say" => Decision::Say(str_at(v, "text")?),
        "wave" => Decision::Wave(EntId(u32_at(v, "ent")?)),
        "idle" => Decision::Idle,
        other => return Err(bad(&format!("decision.type={other}"))),
    })
}

fn parse_routine(v: &Value) -> Result<Routine, String> {
    let arr = v.as_array().ok_or_else(|| bad("routine"))?;
    let mut r = Routine::new(Vec::new());
    for e in arr {
        let minute = e
            .get("minute")
            .and_then(|m| m.as_u64())
            .and_then(|m| u16::try_from(m).ok())
            .filter(|m| *m < 24 * 60)
            .ok_or_else(|| bad("routine.minute"))?;
        let decision = parse_decision(field(e, "decision")?)?;
        r.push(minute, decision);
    }
    Ok(r)
}

/// Parse one payload of `world_input`.
pub fn parse_input(v: &Value) -> Result<BridgeInput, String> {
    let kind = v
        .get("type")
        .and_then(|t| t.as_str())
        .ok_or_else(|| bad("type"))?;
    let world = match kind {
        "spawn" => Input::Spawn {
            ent: EntId(u32_at(v, "ent")?),
            name: str_at(v, "name")?,
            at: tile_at(v, "at")?,
        },
        "despawn" => Input::Despawn {
            ent: EntId(u32_at(v, "ent")?),
        },
        "move" => Input::Move {
            ent: EntId(u32_at(v, "ent")?),
            to: tile_at(v, "to")?,
        },
        "decision" => Input::Decision {
            ent: EntId(u32_at(v, "ent")?),
            decision: parse_decision(field(v, "decision")?)?,
        },
        "place_object" => Input::PlaceObject {
            object: ObjectId(u32_at(v, "object")?),
            kind: str_at(v, "kind")?,
            tile: tile_at(v, "tile")?,
            dir: v
                .get("dir")
                .and_then(|d| d.as_u64())
                .and_then(|d| u8::try_from(d).ok())
                .unwrap_or(0),
            slot: v
                .get("slot")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
        },
        "remove_object" => Input::RemoveObject {
            object: ObjectId(u32_at(v, "object")?),
        },
        "interact" => Input::Interact {
            ent: EntId(u32_at(v, "ent")?),
            object: ObjectId(u32_at(v, "object")?),
        },
        "set_energy" => Input::SetEnergy {
            ent: EntId(u32_at(v, "ent")?),
            energy: u8_at(v, "energy")?,
        },
        "caption" => Input::Caption {
            ent: EntId(u32_at(v, "ent")?),
            text: str_at(v, "text")?,
        },
        "set_routine" => Input::SetRoutine {
            ent: EntId(u32_at(v, "ent")?),
            routine: parse_routine(field(v, "routine")?)?,
        },
        "tick" => Input::Tick,
        // Not a world input: the camera moved, so the diffs this client gets
        // should be filtered to what it can see. `radius` absent or null means
        // "the whole house", which is what the local player wants.
        "interest" => {
            let center = tile_at(v, "center").unwrap_or(Tile::new(0, 0));
            let radius = match v.get("radius") {
                None | Some(Value::Null) => u16::MAX,
                Some(r) => r
                    .as_u64()
                    .and_then(|n| u16::try_from(n).ok())
                    .ok_or_else(|| bad("radius"))?,
            };
            return Ok(BridgeInput::Interest(Interest::new(center, radius)));
        }
        other => return Err(bad(&format!("type={other}"))),
    };
    Ok(BridgeInput::World(Box::new(world)))
}

/// Where one agent works: its own desk, its own workbench, its own chair.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Post {
    /// `object/table`: where a turn is thought through.
    pub desk: Option<ObjectId>,
    /// `object/workbench`: where a tool call is performed.
    pub bench: Option<ObjectId>,
    /// `object/chair`: where a spent budget is sat out.
    pub seat: Option<ObjectId>,
}

/// Where diegetic work happens, resolved from the map's objects and the cast
/// every time either of them changes.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct WorkSpots {
    /// Every `object/workbench`, lowest id first.
    pub bench: Vec<ObjectId>,
    /// Every `object/table`, the one nearest to `bench[i]` at index `i` so an
    /// agent's desk and workbench end up in the same room.
    pub desk: Vec<ObjectId>,
    /// `object/bookshelf`: where a download is put away.
    pub shelf: Option<ObjectId>,
    /// The agent that answers for the house itself when an event names nobody.
    pub host: Option<EntId>,
    /// Roster name to post. Two agents share a post only when the house has
    /// fewer posts than agents; the simulation then stands them side by side.
    pub posts: BTreeMap<String, Post>,
}

impl WorkSpots {
    pub fn post(&self, agent: &str) -> Post {
        self.posts.get(agent).copied().unwrap_or_default()
    }
}

/// FNV-1a. The assignment has to survive a restart, and `DefaultHasher` makes
/// no such promise.
fn stable_hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The index an agent prefers (hash of its name), moved on to the next free
/// one when somebody with a lower entity id already holds it. With more agents
/// than furniture the preferred index is shared.
///
/// Returns the index and whether it had to be shared.
fn claim(
    name: &str,
    len: usize,
    taken: &mut [bool],
    prefer: Option<usize>,
) -> Option<(usize, bool)> {
    if len == 0 {
        return None;
    }
    let start = prefer.unwrap_or((stable_hash(name) % len as u64) as usize);
    for step in 0..len {
        let i = (start + step) % len;
        if !taken[i] {
            taken[i] = true;
            return Some((i, false));
        }
    }
    Some((start, true))
}

/// Pick the work spots out of `(id, kind, tile)` triples, which is what a
/// snapshot gives, and hand every agent of `cast` a post. Agents are walked in
/// entity-id order, so somebody moving in later never takes a post away from
/// somebody who already had one.
pub fn work_spots(objects: &[(ObjectId, String, Tile)], cast: &[(EntId, String)]) -> WorkSpots {
    let of_kind = |k: &str| -> Vec<(ObjectId, Tile)> {
        let mut v: Vec<_> = objects
            .iter()
            .filter(|(_, kind, _)| kind == k)
            .map(|(id, _, tile)| (*id, *tile))
            .collect();
        v.sort_by_key(|(id, _)| *id);
        v
    };
    let benches = of_kind("object/workbench");
    let mut tables = of_kind("object/table");
    let mut desk = Vec::with_capacity(tables.len());
    for (_, at) in &benches {
        if tables.is_empty() {
            break;
        }
        let nearest = (0..tables.len())
            .min_by_key(|i| (tables[*i].1.dist2(*at), tables[*i].0))
            .expect("not empty");
        desk.push(tables.remove(nearest).0);
    }
    desk.extend(tables.into_iter().map(|(id, _)| id));
    let bench: Vec<ObjectId> = benches.into_iter().map(|(id, _)| id).collect();
    let seat: Vec<ObjectId> = of_kind("object/chair")
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let shelf = of_kind("object/bookshelf").first().map(|(id, _)| *id);

    let mut cast: Vec<&(EntId, String)> = cast.iter().collect();
    cast.sort_by_key(|(id, _)| *id);
    let host = cast.first().map(|(id, _)| *id);
    let mut bench_taken = vec![false; bench.len()];
    let mut desk_taken = vec![false; desk.len()];
    let mut seat_taken = vec![false; seat.len()];
    let mut posts = BTreeMap::new();
    for (_, name) in cast {
        let b = claim(name, bench.len(), &mut bench_taken, None);
        // The desk that goes with the bench, when it is still free.
        let d = claim(
            name,
            desk.len(),
            &mut desk_taken,
            b.filter(|(i, shared)| !shared && *i < desk.len())
                .map(|(i, _)| i),
        );
        let s = claim(name, seat.len(), &mut seat_taken, None);
        let desk_id = d.map(|(i, _)| desk[i]);
        let mut bench_id = b.map(|(i, _)| bench[i]);
        // Every workbench is somebody's already: a table nobody thinks at is a
        // better place for tool calls than half of a workbench.
        if matches!(b, Some((_, true))) {
            if let Some((i, false)) = claim(name, desk.len(), &mut desk_taken, None) {
                bench_id = Some(desk[i]);
            }
        }
        posts.insert(
            name.clone(),
            Post {
                // A house with no table thinks at the workbench, and the
                // other way round: nothing is invented, but nothing is lost.
                desk: desk_id.or(bench_id),
                bench: bench_id.or(desk_id),
                seat: s.map(|(i, _)| seat[i]),
            },
        );
    }
    WorkSpots {
        shelf: shelf.or(bench.first().copied()),
        bench,
        desk,
        host,
        posts,
    }
}

/// Energy a spent budget leaves: under the crate's "tired" line (64), so the
/// agent slows down and yawns, and over "spent" (16), so it sits instead of
/// going to bed for good — a budget comes back the next day.
pub const BUDGET_HIT_ENERGY: u8 = 40;

/// Longest caption over an agent's head; anything longer hides the agent.
pub const CAPTION_MAX: usize = 32;

/// `fs_edit cart.js`: the tool and the one argument worth showing. `preview`
/// is what the permission prompt showed (a path, a command, a patch head).
pub fn tool_caption(tool: &str, preview: &str) -> String {
    let first = preview.lines().next().unwrap_or("").trim();
    // A path shows its file name; a command shows its head.
    let arg = if tool.starts_with("fs_") {
        let path = first.split(" (").next().unwrap_or(first);
        path.rsplit(['/', '\\']).next().unwrap_or(path)
    } else {
        first
    };
    let mut out = if arg.is_empty() || arg.starts_with('{') {
        tool.to_string()
    } else {
        format!("{tool} {arg}")
    };
    if out.chars().count() > CAPTION_MAX {
        out = out.chars().take(CAPTION_MAX - 1).collect::<String>() + "…";
    }
    out
}

/// Turn a bus event into world inputs, or nothing.
///
/// `names` maps an agent's roster name to the entity that plays it; an event
/// about an agent with no body in the house is dropped, not guessed. `arg` is
/// what the last permission prompt of that agent showed, for the caption.
pub fn bus_input(
    ev: &BusEvent,
    names: &BTreeMap<String, EntId>,
    spots: &WorkSpots,
    arg: Option<&str>,
) -> Vec<Input> {
    let decide = |ent: EntId, decision: Decision| Input::Decision { ent, decision };
    let ent_of = |agent: &str| names.get(agent).copied();
    match ev {
        BusEvent::TurnStarted { agent, .. } => {
            let (Some(ent), Some(desk)) = (ent_of(agent), spots.post(agent).desk) else {
                return Vec::new();
            };
            vec![decide(ent, Decision::Work(desk))]
        }
        // Waiting for the user: the agent stops where it is and waves at the
        // camera (a wave at oneself faces south).
        BusEvent::ToolAsk {
            agent,
            tool,
            preview,
            ..
        } => {
            let Some(ent) = ent_of(agent) else {
                return Vec::new();
            };
            vec![
                decide(ent, Decision::Wave(ent)),
                Input::Caption {
                    ent,
                    text: format!("{}?", tool_caption(tool, preview)),
                },
            ]
        }
        BusEvent::ToolCalled { agent, tool, .. } => {
            let (Some(ent), Some(bench)) = (ent_of(agent), spots.post(agent).bench) else {
                return Vec::new();
            };
            vec![
                decide(ent, Decision::Work(bench)),
                Input::Caption {
                    ent,
                    text: tool_caption(tool, arg.unwrap_or("")),
                },
            ]
        }
        BusEvent::TurnEnded { agent, .. } => match ent_of(agent) {
            Some(ent) => vec![decide(ent, Decision::Idle)],
            None => Vec::new(),
        },
        // A spent budget is a tired agent: energy is the quota (plan §9.1).
        BusEvent::BudgetHit { agent } => {
            let Some(ent) = ent_of(agent) else {
                return Vec::new();
            };
            let mut out = vec![Input::SetEnergy {
                ent,
                energy: BUDGET_HIT_ENERGY,
            }];
            if let Some(seat) = spots.post(agent).seat {
                out.push(decide(ent, Decision::Sit(seat)));
            }
            out
        }
        // Downloads belong to nobody in particular, so the house agent files
        // them on the shelf.
        BusEvent::DownloadQueued { .. } => match (spots.host, spots.shelf) {
            (Some(host), Some(shelf)) => vec![decide(host, Decision::Work(shelf))],
            _ => Vec::new(),
        },
        BusEvent::DownloadFinished { .. } | BusEvent::DownloadFailed { .. } => match spots.host {
            Some(host) => vec![decide(host, Decision::Idle)],
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Energy from the fraction of a quota window already used. Full window left =
/// 255, window spent = 0; the crate's own thresholds (64 tired, 16 spent) then
/// do the rest without the bridge knowing about them.
pub fn energy_from_used(used: f32) -> u8 {
    let left = (1.0 - used).clamp(0.0, 1.0);
    (left * 255.0).round() as u8
}

/// `SetEnergy` for every agent whose account reports a window. Quotas are
/// `(label, used)` as the observatory's `QuotaStatus` carries them; an account
/// nobody in the house uses is skipped.
pub fn energy_inputs(quotas: &[(String, f32)], names: &BTreeMap<String, EntId>) -> Vec<Input> {
    let mut out = Vec::new();
    for (label, used) in quotas {
        if let Some(ent) = names.get(label) {
            out.push(Input::SetEnergy {
                ent: *ent,
                energy: energy_from_used(*used),
            });
        }
    }
    out.sort_by_key(|i| match i {
        Input::SetEnergy { ent, .. } => ent.0,
        _ => u32::MAX,
    });
    out
}

/// `world_input`: one payload, applied on the next tick.
#[tauri::command]
pub async fn world_input(
    input: serde_json::Value,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    if !super::session::world_enabled(&app) {
        return Err(super::ERR_WORLD_DISABLED.to_string());
    }
    let manager = super::session::manager(&state).ok_or(super::ERR_NO_WORLD)?;
    let parsed = parse_input(&input)?;
    manager.submit(parsed);
    Ok(serde_json::json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniget_core::core::llm::types::Usage;

    fn names() -> BTreeMap<String, EntId> {
        BTreeMap::from([
            ("worker".to_string(), EntId(2)),
            ("omni".to_string(), EntId(1)),
        ])
    }

    fn house() -> Vec<(ObjectId, String, Tile)> {
        vec![
            (ObjectId(9), "object/table".to_string(), Tile::new(26, 3)),
            (
                ObjectId(6),
                "object/workbench".to_string(),
                Tile::new(18, 3),
            ),
            (
                ObjectId(7),
                "object/workbench".to_string(),
                Tile::new(19, 16),
            ),
            (ObjectId(12), "object/table".to_string(), Tile::new(23, 20)),
            (
                ObjectId(8),
                "object/bookshelf".to_string(),
                Tile::new(16, 1),
            ),
            (ObjectId(5), "object/chair".to_string(), Tile::new(21, 3)),
            (ObjectId(2), "object/lamp".to_string(), Tile::new(26, 3)),
        ]
    }

    fn cast() -> Vec<(EntId, String)> {
        vec![
            (EntId(2), "worker".to_string()),
            (EntId(1), "omni".to_string()),
        ]
    }

    fn spots() -> WorkSpots {
        work_spots(&house(), &cast())
    }

    #[test]
    fn a_click_to_walk_parses() {
        let got =
            parse_input(&serde_json::json!({ "type": "move", "ent": 3, "to": [4, 5] })).unwrap();
        assert_eq!(
            got,
            BridgeInput::World(Box::new(Input::Move {
                ent: EntId(3),
                to: Tile::new(4, 5)
            }))
        );
    }

    #[test]
    fn a_tile_parses_from_both_spellings() {
        let a =
            parse_input(&serde_json::json!({ "type": "move", "ent": 1, "to": [1, 2] })).unwrap();
        let b = parse_input(&serde_json::json!({ "type": "move", "ent": 1, "to": {"x":1,"y":2} }))
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn every_decision_shape_parses() {
        let cases: Vec<(serde_json::Value, Decision)> = vec![
            (
                serde_json::json!({"type":"go_to","to":[1,2]}),
                Decision::GoTo(Tile::new(1, 2)),
            ),
            (
                serde_json::json!({"type":"sit","object":4}),
                Decision::Sit(ObjectId(4)),
            ),
            (serde_json::json!({"type":"sleep"}), Decision::Sleep),
            (
                serde_json::json!({"type":"work","object":6}),
                Decision::Work(ObjectId(6)),
            ),
            (
                serde_json::json!({"type":"say","text":"oi"}),
                Decision::Say("oi".into()),
            ),
            (
                serde_json::json!({"type":"wave","ent":2}),
                Decision::Wave(EntId(2)),
            ),
            (serde_json::json!({"type":"idle"}), Decision::Idle),
        ];
        for (json, want) in cases {
            assert_eq!(parse_decision(&json).unwrap(), want, "for {json}");
        }
    }

    #[test]
    fn spawn_place_and_routine_parse_with_their_optional_fields() {
        let spawn =
            parse_input(&serde_json::json!({"type":"spawn","ent":1,"name":"Omni","at":[2,3]}))
                .unwrap();
        let BridgeInput::World(b) = spawn else {
            panic!("a spawn is a world input");
        };
        assert!(matches!(*b, Input::Spawn { .. }));

        let place = parse_input(&serde_json::json!({
            "type": "place_object", "object": 30, "kind": "object/lamp", "tile": [4, 2]
        }))
        .unwrap();
        assert_eq!(
            place,
            BridgeInput::World(Box::new(Input::PlaceObject {
                object: ObjectId(30),
                kind: "object/lamp".into(),
                tile: Tile::new(4, 2),
                dir: 0,
                slot: None,
            }))
        );

        let routine = parse_input(&serde_json::json!({
            "type": "set_routine", "ent": 1,
            "routine": [{ "minute": 480, "decision": { "type": "work", "object": 6 } }]
        }))
        .unwrap();
        let BridgeInput::World(b) = routine else {
            panic!("world input");
        };
        let Input::SetRoutine { routine, .. } = *b else {
            panic!("routine");
        };
        assert_eq!(routine.len(), 1);
    }

    #[test]
    fn interest_is_not_a_world_input() {
        let all = parse_input(&serde_json::json!({ "type": "interest" })).unwrap();
        assert_eq!(all, BridgeInput::Interest(Interest::ALL));
        let near =
            parse_input(&serde_json::json!({ "type": "interest", "center": [8, 8], "radius": 12 }))
                .unwrap();
        assert_eq!(
            near,
            BridgeInput::Interest(Interest::new(Tile::new(8, 8), 12))
        );
    }

    #[test]
    fn a_bad_payload_says_which_field_and_never_panics() {
        for (json, needle) in [
            (serde_json::json!({}), "type"),
            (serde_json::json!({ "type": "nope" }), "type=nope"),
            (serde_json::json!({ "type": "move", "ent": 1 }), "to"),
            (serde_json::json!({ "type": "move", "to": [1, 2] }), "ent"),
            (
                serde_json::json!({ "type": "move", "ent": 1, "to": [1] }),
                "to",
            ),
            (
                serde_json::json!({ "type": "set_energy", "ent": 1, "energy": 900 }),
                "energy",
            ),
            (
                serde_json::json!({ "type": "decision", "ent": 1, "decision": { "type": "fly" } }),
                "decision.type=fly",
            ),
            (
                serde_json::json!({ "type": "set_routine", "ent": 1, "routine": [{ "minute": 2000, "decision": {"type":"idle"} }] }),
                "routine.minute",
            ),
        ] {
            let err = parse_input(&json).unwrap_err();
            assert!(err.starts_with("ERR_WORLD_BAD_INPUT"), "for {json}: {err}");
            assert!(err.contains(needle), "for {json}: {err}");
        }
    }

    #[test]
    fn work_spots_come_from_the_map_and_are_stable() {
        let s = spots();
        assert_eq!(s.bench, vec![ObjectId(6), ObjectId(7)]);
        // Each desk is the table nearest to the bench of the same index.
        assert_eq!(s.desk, vec![ObjectId(9), ObjectId(12)]);
        assert_eq!(s.shelf, Some(ObjectId(8)));
        assert_eq!(s.host, Some(EntId(1)));
        assert_eq!(s, spots(), "the same house and cast give the same posts");
        // No workbench in the house: nothing is invented.
        let bare = work_spots(
            &[(ObjectId(2), "object/lamp".into(), Tile::new(1, 1))],
            &cast(),
        );
        assert_eq!(bare.post("worker"), Post::default());
        assert!(bare.bench.is_empty() && bare.desk.is_empty());
    }

    #[test]
    fn two_agents_never_share_a_post_while_the_house_has_enough() {
        let s = spots();
        let (a, b) = (s.post("omni"), s.post("worker"));
        assert!(a.bench.is_some() && a.desk.is_some());
        assert_ne!(a.bench, b.bench);
        assert_ne!(a.desk, b.desk);
        // The desk goes with the bench: same index, so same room.
        for p in [a, b] {
            let i = s.bench.iter().position(|o| Some(*o) == p.bench).unwrap();
            assert_eq!(Some(s.desk[i]), p.desk);
        }
    }

    #[test]
    fn a_newcomer_never_takes_a_post_from_somebody_who_had_one() {
        let before = spots();
        let mut bigger = cast();
        bigger.push((EntId(3), "scout".to_string()));
        bigger.push((EntId(4), "fourth".to_string()));
        let after = work_spots(&house(), &bigger);
        assert_eq!(after.post("omni"), before.post("omni"));
        assert_eq!(after.post("worker"), before.post("worker"));
        // More agents than furniture: the post is shared, never missing. The
        // simulation stands the second one on the free tile beside it.
        assert!(after.post("scout").bench.is_some());
        assert!(after.post("fourth").desk.is_some());
    }

    #[test]
    fn a_third_agent_takes_a_spare_table_before_it_shares_a_workbench() {
        let mut objects = house();
        objects.push((ObjectId(20), "object/table".to_string(), Tile::new(7, 23)));
        objects.push((ObjectId(3), "object/table".to_string(), Tile::new(6, 2)));
        let mut three = cast();
        three.push((EntId(3), "scout".to_string()));
        let s = work_spots(&objects, &three);
        let posts = [s.post("omni"), s.post("worker"), s.post("scout")];
        let mut spots: Vec<ObjectId> = posts
            .iter()
            .flat_map(|p| [p.desk.unwrap(), p.bench.unwrap()])
            .collect();
        spots.sort();
        spots.dedup();
        assert_eq!(
            spots.len(),
            6,
            "three agents, six different pieces of furniture"
        );
    }

    #[test]
    fn a_tool_call_sends_that_agent_to_its_workbench_with_a_caption() {
        let s = spots();
        let got = bus_input(
            &BusEvent::ToolCalled {
                agent: "worker".into(),
                tool: "fs_edit".into(),
                ok: true,
                ms: 12,
            },
            &names(),
            &s,
            Some("src/shop/cart.js\n- a\n+ b"),
        );
        assert_eq!(
            got,
            vec![
                Input::Decision {
                    ent: EntId(2),
                    decision: Decision::Work(s.post("worker").bench.unwrap())
                },
                Input::Caption {
                    ent: EntId(2),
                    text: "fs_edit cart.js".into()
                },
            ]
        );
    }

    #[test]
    fn a_caption_is_the_tool_and_its_main_argument_and_stays_short() {
        assert_eq!(tool_caption("shell_exec", ""), "shell_exec");
        assert_eq!(
            tool_caption("shell_exec", "npm test\n--watch"),
            "shell_exec npm test"
        );
        assert_eq!(
            tool_caption("fs_write", "/w/src/cart.js (120 bytes)"),
            "fs_write cart.js"
        );
        assert_eq!(tool_caption("dl_add", "{\"url\":\"x\"}"), "dl_add");
        let long = tool_caption("shell_exec", &"x".repeat(200));
        assert_eq!(long.chars().count(), CAPTION_MAX);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn a_permission_prompt_stops_the_agent_and_waves_at_the_camera() {
        let got = bus_input(
            &BusEvent::ToolAsk {
                agent: "worker".into(),
                request_id: "r1".into(),
                tool_call_id: "t1".into(),
                tool: "shell_exec".into(),
                preview: "npm test".into(),
            },
            &names(),
            &spots(),
            None,
        );
        assert_eq!(
            got,
            vec![
                Input::Decision {
                    ent: EntId(2),
                    decision: Decision::Wave(EntId(2))
                },
                Input::Caption {
                    ent: EntId(2),
                    text: "shell_exec npm test?".into()
                },
            ]
        );
    }

    #[test]
    fn a_turn_sends_that_agent_to_its_desk_and_frees_it_at_the_end() {
        let s = spots();
        assert_eq!(
            bus_input(
                &BusEvent::TurnStarted {
                    agent: "worker".into(),
                    conversation: "c1".into()
                },
                &names(),
                &s,
                None,
            ),
            vec![Input::Decision {
                ent: EntId(2),
                decision: Decision::Work(s.post("worker").desk.unwrap())
            }]
        );
        assert_eq!(
            bus_input(
                &BusEvent::TurnEnded {
                    agent: "worker".into(),
                    usage: Usage::default()
                },
                &names(),
                &s,
                None,
            ),
            vec![Input::Decision {
                ent: EntId(2),
                decision: Decision::Idle
            }]
        );
    }

    #[test]
    fn a_download_is_filed_by_the_house_agent() {
        assert_eq!(
            bus_input(
                &BusEvent::DownloadQueued { id: 7 },
                &names(),
                &spots(),
                None
            ),
            vec![Input::Decision {
                ent: EntId(1),
                decision: Decision::Work(ObjectId(8))
            }]
        );
    }

    #[test]
    fn a_spent_budget_sits_the_agent_down_tired() {
        assert_eq!(
            bus_input(
                &BusEvent::BudgetHit {
                    agent: "worker".into()
                },
                &names(),
                &spots(),
                None,
            ),
            vec![
                Input::SetEnergy {
                    ent: EntId(2),
                    energy: BUDGET_HIT_ENERGY
                },
                Input::Decision {
                    ent: EntId(2),
                    decision: Decision::Sit(ObjectId(5))
                },
            ]
        );
    }

    #[test]
    fn an_event_about_a_stranger_or_a_missing_spot_is_dropped() {
        let ev = BusEvent::ToolCalled {
            agent: "nobody".into(),
            tool: "x".into(),
            ok: true,
            ms: 1,
        };
        assert_eq!(bus_input(&ev, &names(), &spots(), None), Vec::new());
        let ev = BusEvent::ToolCalled {
            agent: "worker".into(),
            tool: "x".into(),
            ok: true,
            ms: 1,
        };
        assert_eq!(
            bus_input(&ev, &names(), &WorkSpots::default(), None),
            Vec::new()
        );
        // Events the world has no opinion about stay out of it.
        assert_eq!(
            bus_input(
                &BusEvent::TokenDelta {
                    agent: "worker".into(),
                    chars: 3
                },
                &names(),
                &spots(),
                None,
            ),
            Vec::new()
        );
    }

    #[test]
    fn energy_is_what_is_left_of_the_window() {
        assert_eq!(energy_from_used(0.0), 255);
        assert_eq!(energy_from_used(1.0), 0);
        assert_eq!(energy_from_used(0.5), 128);
        // Out-of-range numbers from a provider must not wrap.
        assert_eq!(energy_from_used(-3.0), 255);
        assert_eq!(energy_from_used(9.0), 0);
        assert_eq!(energy_from_used(f32::NAN), 0);
    }

    #[test]
    fn energy_inputs_cover_only_the_accounts_the_house_uses() {
        let got = energy_inputs(
            &[
                ("worker".into(), 0.75),
                ("unused-account".into(), 0.1),
                ("omni".into(), 0.0),
            ],
            &names(),
        );
        assert_eq!(
            got,
            vec![
                Input::SetEnergy {
                    ent: EntId(1),
                    energy: 255
                },
                Input::SetEnergy {
                    ent: EntId(2),
                    energy: 64
                },
            ]
        );
    }
}
