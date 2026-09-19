//! Persistence of the world. Owned by f7-world-bridge.
//!
//! One file, `<app_data>/world/save.bin`, holding the crate's own binary
//! snapshot — the same bytes that go down the channel, so there is exactly one
//! format to keep working. Written at most every 30 s and only when the world
//! is dirty, and once more on close, in the mould of `core::queue`'s
//! save-if-dirty.
//!
//! Restoring needs a little care: the crate gives a replica `World::apply`, not
//! a `World::from_snapshot`. [`restore`] builds the one diff that turns a fresh
//! world of the same seed and map into the saved one — every agent as a spawn
//! delta, every object the save has as a placement and every map object the
//! save does *not* have as a removal — and applies it. `map_hash` does the
//! checking for us: a save from another house is refused with
//! `ERR_WORLD_MAP_MISMATCH` instead of producing a scrambled room.

use std::path::{Path, PathBuf};

use omniget_world::snapshot::diff::ObjDelta;
use omniget_world::{Diff, EntDelta, MapDef, Snapshot, World};

/// Milliseconds between autosaves of a dirty world.
pub const SAVE_INTERVAL_MS: u64 = 30_000;

/// Biggest save file we will read back. A 64-agent house snapshots at about
/// 1.4 KB; 8 MiB is four orders of magnitude of headroom and stops a corrupt
/// path from turning into an allocation.
pub const MAX_SAVE_BYTES: u64 = 8 * 1024 * 1024;

/// `<root>/world`.
pub fn world_dir(root: &Path) -> PathBuf {
    root.join("world")
}

/// `<root>/world/save.bin`.
pub fn save_path(root: &Path) -> PathBuf {
    world_dir(root).join("save.bin")
}

/// Is there a world on disk.
pub fn save_exists(root: &Path) -> bool {
    save_path(root).is_file()
}

/// Write the snapshot bytes, atomically: a crash halfway through a save must
/// not leave a truncated file where a world used to be.
pub fn write_save(root: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = world_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("ERR_WORLD_SAVE: {e}"))?;
    let final_path = save_path(root);
    let tmp = dir.join("save.bin.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("ERR_WORLD_SAVE: {e}"))?;
    std::fs::rename(&tmp, &final_path).map_err(|e| format!("ERR_WORLD_SAVE: {e}"))?;
    Ok(())
}

/// Read the snapshot bytes back, or `None` when there is no world.
pub fn read_save(root: &Path) -> Option<Vec<u8>> {
    let path = save_path(root);
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_SAVE_BYTES {
        return None;
    }
    std::fs::read(&path).ok()
}

/// Remove the whole world directory: save, profile and anything a neighbour
/// put beside them (the brain's `memory.sqlite`). "Delete my world" means all
/// of it.
pub fn delete_world(root: &Path) -> Result<(), String> {
    let dir = world_dir(root);
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("ERR_WORLD_SAVE: {e}"))
}

/// Is an autosave due. Pure so the thread's clock is testable.
pub const fn save_due(dirty: bool, since_last_save_ms: u64) -> bool {
    dirty && since_last_save_ms >= SAVE_INTERVAL_MS
}

/// Rebuild a world from a decoded snapshot.
///
/// The caller supplies the map, because the snapshot carries only its hash.
/// A mismatch is the crate's own `ERR_WORLD_MAP_MISMATCH`.
pub fn restore(snap: &Snapshot, map: MapDef) -> Result<World, String> {
    let mut world = World::new(snap.seed, map).map_err(|e| format!("{}: {e}", e.code()))?;
    if world.tick() != 0 {
        return Err("ERR_WORLD_SAVE: a fresh world is not at tick 0".into());
    }
    let diff = diff_from_snapshot(&world, snap);
    world
        .apply(&diff)
        .map_err(|e| format!("{}: {e}", e.code()))?;
    Ok(world)
}

/// The diff that turns `fresh` into `snap`. Public for the test that proves
/// a round trip, not for callers.
fn diff_from_snapshot(fresh: &World, snap: &Snapshot) -> Diff {
    let mut objects: Vec<ObjDelta> = Vec::new();
    // Anything the fresh world got from the map but the save does not have was
    // removed by the player; put the removals first so a re-place of the same
    // id in the same pass still wins.
    let saved_ids: Vec<u32> = snap.objects.iter().map(|o| o.id.0).collect();
    for o in fresh.snapshot().objects {
        if !saved_ids.contains(&o.id.0) {
            objects.push(ObjDelta::Removed(o.id));
        }
    }
    for o in &snap.objects {
        objects.push(ObjDelta::Placed(o.clone()));
    }
    Diff {
        from: fresh.tick(),
        to: snap.tick,
        map_hash: snap.map_hash,
        rng_state: snap.rng_state,
        sleep: snap.sleep,
        ents: snap.agents.iter().map(EntDelta::spawned).collect(),
        objects,
        events: Vec::new(),
    }
}

/// Decode and restore in one step, with the decoder's own error codes.
pub fn restore_bytes(bytes: &[u8], map: MapDef) -> Result<World, String> {
    let snap = Snapshot::decode(bytes).map_err(|e| format!("{}: {e}", e.code()))?;
    restore(&snap, map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::world::session::test_map;
    use omniget_world::{Decision, EntId, Input, ObjectId, Tile};

    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("omniget-world-save-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn paths_are_built_with_join_not_slashes() {
        let root = Path::new("root");
        assert_eq!(world_dir(root), root.join("world"));
        assert_eq!(save_path(root), root.join("world").join("save.bin"));
    }

    #[test]
    fn a_save_round_trips_through_the_disk() {
        let root = tmp_root("round-trip");
        assert!(!save_exists(&root));
        assert_eq!(read_save(&root), None);
        write_save(&root, &[1, 2, 3, 4]).unwrap();
        assert!(save_exists(&root));
        assert_eq!(read_save(&root).unwrap(), vec![1, 2, 3, 4]);
        // No temporary file left behind.
        assert!(!world_dir(&root).join("save.bin.tmp").exists());
        delete_world(&root).unwrap();
        assert!(!save_exists(&root));
        // Deleting twice is not an error.
        delete_world(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn autosave_waits_for_dirt_and_for_thirty_seconds() {
        assert!(!save_due(false, 60_000));
        assert!(!save_due(true, SAVE_INTERVAL_MS - 1));
        assert!(save_due(true, SAVE_INTERVAL_MS));
    }

    #[test]
    fn a_restored_world_is_the_same_world() {
        let mut w = World::new(7, test_map()).unwrap();
        w.step(&[Input::Spawn {
            ent: EntId(1),
            name: "Omni".into(),
            at: Tile::new(2, 2),
        }]);
        w.step(&[Input::Spawn {
            ent: EntId(2),
            name: "Ana".into(),
            at: Tile::new(3, 3),
        }]);
        w.step(&[Input::SetEnergy {
            ent: EntId(1),
            energy: 40,
        }]);
        w.step(&[Input::Decision {
            ent: EntId(2),
            decision: Decision::Sleep,
        }]);
        for _ in 0..12 {
            w.step(&[Input::Tick]);
        }
        let before = w.snapshot();
        let bytes = before.encode();

        let restored = restore_bytes(&bytes, test_map()).unwrap();
        let after = restored.snapshot();
        assert_eq!(
            after, before,
            "snapshot survives save and load byte for byte"
        );
        assert_eq!(restored.tick(), w.tick());
        assert_eq!(restored.seed(), 7);
        assert_eq!(restored.agent_count(), 2);
        assert_eq!(restored.energy_of(EntId(1)), Some(40));
    }

    #[test]
    fn a_restored_world_keeps_objects_the_player_removed() {
        let mut w = World::new(1, test_map()).unwrap();
        let victim = w.snapshot().objects.first().map(|o| o.id).unwrap();
        w.step(&[Input::RemoveObject { object: victim }]);
        let before = w.snapshot();
        let restored = restore_bytes(&before.encode(), test_map()).unwrap();
        assert_eq!(restored.snapshot(), before);
        assert!(
            restored.snapshot().objects.iter().all(|o| o.id != victim),
            "a removed object must not come back from the map"
        );
    }

    #[test]
    fn a_restored_world_keeps_objects_the_player_added() {
        let mut w = World::new(1, test_map()).unwrap();
        let n = w.object_count();
        w.step(&[Input::PlaceObject {
            object: ObjectId(900),
            kind: "object/chair".into(),
            tile: Tile::new(4, 4),
            dir: 0,
            slot: None,
        }]);
        assert_eq!(w.object_count(), n + 1);
        let before = w.snapshot();
        let restored = restore_bytes(&before.encode(), test_map()).unwrap();
        assert_eq!(restored.snapshot(), before);
    }

    #[test]
    fn a_save_from_another_house_is_refused_not_scrambled() {
        let w = World::new(1, test_map()).unwrap();
        let mut snap = w.snapshot();
        snap.map_hash ^= 0xdead_beef;
        let err = restore(&snap, test_map()).unwrap_err();
        assert!(err.starts_with("ERR_WORLD_MAP_MISMATCH"), "got {err}");
    }

    #[test]
    fn garbage_bytes_are_refused_with_the_decoder_code() {
        let err = restore_bytes(&[0, 1, 2, 3, 4, 5, 6], test_map()).unwrap_err();
        assert!(err.starts_with("ERR_WORLD_BAD_MAGIC"), "got {err}");
        let err = restore_bytes(&[], test_map()).unwrap_err();
        assert!(err.starts_with("ERR_WORLD_"), "got {err}");
    }

    #[test]
    fn an_oversized_file_is_not_read_into_memory() {
        let root = tmp_root("oversized");
        std::fs::create_dir_all(world_dir(&root)).unwrap();
        let big = vec![0u8; (MAX_SAVE_BYTES + 1) as usize];
        std::fs::write(save_path(&root), &big).unwrap();
        assert_eq!(read_save(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
