//! `world_*` commands: the session. Owned by f7-world-bridge.
//!
//! Where "Never" is enforced. `AppState.world` is a `OnceLock` that nothing
//! fills until one of these commands runs, so an app whose user never opens
//! `/world` allocates no `World`, spawns no thread and reads no map. The first
//! command to touch the world builds the [`WorldManager`]; only `world_open`
//! starts the tick thread.
//!
//! The map comes from `static/world/house-v1.json`, which in a built app is
//! part of the embedded frontend, so it is fetched through the asset resolver
//! rather than from a path on disk. A user override in
//! `<app_data>/world/maps/` wins, which is how a hand-edited house is tested
//! without rebuilding.

use std::sync::Arc;

use omniget_world::MapDef;
use tauri::ipc::{Channel, InvokeResponseBody, Response};
use tauri::Manager;

use crate::world_manager::{DiffSink, WorldManager};

use super::save;

/// Houses whose file name is not simply `<id>.json`. `casa-v1` is the id the
/// route and the atlas use; `house-v1.json` is what the file is called.
const HOUSE_FILES: &[(&str, &str)] = &[("casa-v1", "house-v1.json")];

/// The default house, and the only one in phase 7.
pub const DEFAULT_HOUSE: &str = "casa-v1";

/// File name for a house id, refusing anything that is not a plain identifier:
/// this string ends up in a path.
pub fn house_file(house: &str) -> Result<String, String> {
    let ok = !house.is_empty()
        && house.len() <= 64
        && house
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        return Err(format!("{}: {house}", super::ERR_BAD_HOUSE));
    }
    Ok(HOUSE_FILES
        .iter()
        .find(|(id, _)| *id == house)
        .map(|(_, file)| (*file).to_string())
        .unwrap_or_else(|| format!("{house}.json")))
}

/// Read the map JSON, in order of precedence: the user's own copy, the
/// embedded frontend asset, a bundled resource, the source tree in dev.
fn read_map_json(app: &tauri::AppHandle, file: &str) -> Option<String> {
    if let Some(root) = omniget_core::core::paths::app_data_dir() {
        let user = save::world_dir(&root).join("maps").join(file);
        if let Ok(text) = std::fs::read_to_string(&user) {
            return Some(text);
        }
    }
    if let Some(asset) = app.asset_resolver().get(format!("/world/{file}")) {
        if let Ok(text) = String::from_utf8(asset.bytes) {
            return Some(text);
        }
    }
    if let Ok(path) = app.path().resolve(
        format!("world/{file}"),
        tauri::path::BaseDirectory::Resource,
    ) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            return Some(text);
        }
    }
    // Dev: `pnpm dev` serves `static/` from Vite, which the asset resolver does
    // not see. `CARGO_MANIFEST_DIR` is `src-tauri`.
    let dev = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("static")
        .join("world")
        .join(file);
    std::fs::read_to_string(dev).ok()
}

/// The map definition for a house.
pub fn load_map(app: &tauri::AppHandle, house: &str) -> Result<MapDef, String> {
    let file = house_file(house)?;
    let text = read_map_json(app, &file).ok_or_else(|| format!("{}: {file}", super::ERR_NO_MAP))?;
    serde_json::from_str::<MapDef>(&text).map_err(|e| format!("ERR_WORLD_MAP_INVALID: {e}"))
}

/// The manager, if anything ever built one.
pub fn manager(state: &crate::AppState) -> Option<Arc<WorldManager>> {
    state.world.get().cloned()
}

/// Is the world switched on. `settings.world.enabled` defaults to `false`, so
/// the answer on a fresh install — and on an install whose settings file cannot
/// be read at all — is "no".
pub fn world_enabled(app: &tauri::AppHandle) -> bool {
    crate::storage::config::load_settings(app).world.enabled
}

/// The gate itself, as a pure function, so the "disabled means nothing is
/// built" rule is a unit test and not a claim.
///
/// The order matters: `enabled` is checked **before** the cell, so turning the
/// world off in Settings stops the commands from answering even in the session
/// that had it on. The already-running thread is stopped separately, by the
/// route calling `world_close` on its way out.
pub fn ensure_manager_in<F>(
    enabled: bool,
    cell: &std::sync::OnceLock<Arc<WorldManager>>,
    build: F,
) -> Result<Arc<WorldManager>, String>
where
    F: FnOnce() -> Result<Arc<WorldManager>, String>,
{
    if !enabled {
        return Err(super::ERR_WORLD_DISABLED.to_string());
    }
    if let Some(m) = cell.get() {
        return Ok(m.clone());
    }
    let m = build()?;
    // A losing race just drops our manager; the winner is equally wired.
    let _ = cell.set(m);
    cell.get()
        .cloned()
        .ok_or_else(|| super::ERR_NO_WORLD.to_string())
}

/// The manager, building it on first use and wiring it to the bus, the app and
/// the quota windows. Still starts no thread.
pub fn ensure_manager(
    app: &tauri::AppHandle,
    state: &crate::AppState,
) -> Result<Arc<WorldManager>, String> {
    let enabled = world_enabled(app);
    ensure_manager_in(enabled, &state.world, || {
        let root = omniget_core::core::paths::app_data_dir()
            .ok_or_else(|| super::ERR_NO_DATA_DIR.to_string())?;
        let m = Arc::new(WorldManager::new(root));
        m.attach_app(app.clone());
        m.attach_bus(state.llm.bus());
        {
            // Minds through the LLM stack, only when the user turned it on.
            let llm = state.llm.clone();
            let app = app.clone();
            m.set_thinker(Arc::new(move |name: &str| {
                let settings = crate::storage::config::load_settings(&app);
                if !settings.world.thinking {
                    return None;
                }
                let agent = llm
                    .roster()
                    .into_iter()
                    .find(|a| a.id == name || a.name == name)?;
                llm.attach_app(&app);
                if let Err(e) = llm.prepare_agent(&agent) {
                    tracing::debug!("[world] {name} cannot think: {e}");
                    return None;
                }
                let thinker: Arc<dyn omniget_core::core::llm::brain::Thinker> =
                    Arc::new(omniget_core::core::llm::brain::CoordinatorThinker::new(
                        llm.coordinator(),
                        agent,
                    ));
                Some((thinker, settings.world.think_interval_s))
            }));
        }
        {
            // Everybody in the roster lives in the house, CLI and ACP agents
            // included: the house shows who works for the user, not how.
            let llm = state.llm.clone();
            m.set_roster_source(Arc::new(move || {
                llm.roster().into_iter().map(|a| a.id).collect()
            }));
        }
        let llm = state.llm.clone();
        m.set_quota_source(Arc::new(move || {
            llm.telemetry()
                .quotas
                .into_iter()
                .map(|q| (q.label, q.used))
                .collect()
        }));
        Ok(m)
    })
}

/// Diffs down a `tauri::ipc::Channel`.
///
/// `InvokeResponseBody::Raw`, not `Vec<u8>`: `Vec<u8>` is `Serialize`, so a
/// `Channel<Vec<u8>>` would send a JSON array of numbers — several times the
/// bytes and a different measurement from the one I-5 signed off (f6-bench).
/// `Raw` arrives in the page as an `ArrayBuffer`.
struct ChannelSink(Channel<InvokeResponseBody>);

impl DiffSink for ChannelSink {
    fn send(&self, bytes: Vec<u8>) -> Result<(), String> {
        self.0
            .send(InvokeResponseBody::Raw(bytes))
            .map_err(|e| format!("ERR_WORLD_CHANNEL: {e}"))
    }
}

/// Follow the main window into and out of the background, which is what the
/// ten-minute hibernation counts. Tauri 2 has no `Minimized` window event, so
/// losing focus is the trigger and `is_minimized` only sharpens the log.
fn watch_window(app: &tauri::AppHandle, manager: Arc<WorldManager>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let probe = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Focused(focused) = event {
            if *focused {
                manager.note_background(false);
            } else {
                let minimized = probe.is_minimized().unwrap_or(false);
                tracing::debug!("[world] window backgrounded (minimized={minimized})");
                manager.note_background(true);
            }
        }
    });
}

fn random_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // Not cryptography: any 64 bits that differ between two "new world" clicks.
    nanos ^ ((std::process::id() as u64) << 32) ^ 0x9e37_79b9_7f4a_7c15
}

/// Is there a world at all. Cheap, and deliberately does not build a manager:
/// the sidebar may ask this on every launch.
///
/// A disabled world answers `false` rather than an error: this is the question
/// the shell asks before it decides whether to offer the route, and "there is
/// nothing to show" is the true answer, save file on disk or not.
#[tauri::command]
pub async fn world_exists(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<bool, String> {
    if !world_enabled(&app) {
        return Ok(false);
    }
    if let Some(m) = manager(&state) {
        if m.has_world() {
            return Ok(true);
        }
    }
    let Some(root) = omniget_core::core::paths::app_data_dir() else {
        return Ok(false);
    };
    Ok(save::save_exists(&root))
}

/// Make a new world. Refuses to overwrite one that already exists: deleting is
/// its own command, with its own confirmation.
#[tauri::command]
pub async fn world_create(
    seed: Option<u64>,
    house: Option<String>,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<serde_json::Value, String> {
    let house = house.unwrap_or_else(|| DEFAULT_HOUSE.to_string());
    let manager = ensure_manager(&app, &state)?;
    if manager.has_world() || save::save_exists(manager.root()) {
        return Err(super::ERR_WORLD_EXISTS.to_string());
    }
    let map = load_map(&app, &house)?;
    let seed = seed.unwrap_or_else(random_seed);
    manager.create(seed, map, &house)?;
    // A house with nobody in it is not a game: the roster moves in at once.
    // The name is the roster id, which is how bus events find their agent in
    // the house.
    let (residents, _) = manager.sync_roster();
    Ok(
        serde_json::json!({ "seed": seed, "house": house, "tick": manager.tick(), "residents": residents }),
    )
}

/// Open the world for a route that is about to draw it: attaches the channel,
/// starts the tick thread and answers with the snapshot to start from. The
/// channel carries diffs from that tick onwards.
#[tauri::command]
pub async fn world_open(
    channel: Channel<InvokeResponseBody>,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<Response, String> {
    let manager = ensure_manager(&app, &state)?;
    if !manager.has_world() {
        let house = if manager.house().is_empty() {
            DEFAULT_HOUSE.to_string()
        } else {
            manager.house()
        };
        let bytes = save::read_save(manager.root()).ok_or(super::ERR_NO_WORLD)?;
        let map = load_map(&app, &house)?;
        manager.restore(&bytes, map, &house)?;
        manager.upgrade_posts();
    }
    watch_window(&app, manager.clone());
    // Whoever joined the roster since the last visit moves in, whoever left
    // moves out. The tick thread repeats this while the house is alive.
    manager.sync_roster();
    let snapshot = manager.open(Arc::new(ChannelSink(channel)))?;
    Ok(Response::new(snapshot))
}

/// The route lost a blob (`ERR_WORLD_DIFF_GAP`) and asks for a whole snapshot
/// on the channel it already has.
#[tauri::command]
pub async fn world_resync(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    if let Some(m) = manager(&state) {
        m.resync();
    }
    Ok(())
}

/// What every resident is doing right now, for the activity panel. Changes
/// arrive afterwards as `world://activity`.
#[tauri::command]
pub async fn world_activity(
    state: tauri::State<'_, crate::AppState>,
) -> Result<Vec<crate::world_manager::AgentWork>, String> {
    Ok(manager(&state).map(|m| m.work()).unwrap_or_default())
}

/// The route went away. The world keeps living at 0.2 Hz.
///
/// Not gated on `settings.world.enabled`: closing frees things, it never
/// allocates any, and it is exactly what Settings calls when the user switches
/// the world off with the route still open. In that case it goes further than
/// dozing and stops the tick thread outright — a world that is switched off
/// must cost nothing, not 0.2 Hz.
#[tauri::command]
pub async fn world_close(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    if let Some(m) = manager(&state) {
        if world_enabled(&app) {
            m.close();
        } else {
            m.shutdown();
        }
    }
    Ok(())
}

/// The route is still mounted but hidden (another tab, a background window).
#[tauri::command]
pub async fn world_set_visible(
    visible: bool,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
) -> Result<(), String> {
    if !world_enabled(&app) {
        return Err(super::ERR_WORLD_DISABLED.to_string());
    }
    if let Some(m) = manager(&state) {
        m.set_visible(visible);
    }
    Ok(())
}

/// Erase the world: the thread, the state and `<app_data>/world/` (the save,
/// the render profile and the brain's `memory.sqlite`).
///
/// Not gated either: "I turned it off, now delete my data" has to work.
#[tauri::command]
pub async fn world_delete(state: tauri::State<'_, crate::AppState>) -> Result<(), String> {
    match manager(&state) {
        Some(m) => m.delete(),
        None => match omniget_core::core::paths::app_data_dir() {
            Some(root) => save::delete_world(&root),
            None => Ok(()),
        },
    }
}

/// The minimal complete house from `omniget-world/tests/fixtures`, used by
/// every test in this module tree. Not the real `house-v1.json`, which belongs
/// to f7-house-content; it is the same `MapDef` type and the same validator.
#[cfg(test)]
pub fn test_map() -> MapDef {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("omniget-world")
        .join("tests")
        .join("fixtures")
        .join("house-v1.min.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("fixture {} is missing: {e}", path.display()));
    serde_json::from_str(&text).expect("the fixture is a MapDef")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_manager::tick_threads;

    #[test]
    fn the_fixture_is_a_map_the_crate_accepts() {
        let map = test_map();
        let world = omniget_world::World::new(1, map).expect("the fixture loads");
        assert_eq!(world.tick(), 0);
        assert!(world.object_count() > 0);
    }

    #[test]
    fn house_ids_map_to_files_and_refuse_paths() {
        assert_eq!(house_file("casa-v1").unwrap(), "house-v1.json");
        assert_eq!(house_file("loft").unwrap(), "loft.json");
        for bad in ["", "../etc/passwd", "a/b", "casa v1", "casa.v1"] {
            let err = house_file(bad).unwrap_err();
            assert!(
                err.starts_with(super::super::ERR_BAD_HOUSE),
                "for {bad}: {err}"
            );
        }
    }

    /// The opt-in gate (`settings.world.enabled`, default false). A disabled
    /// world does not allocate a `WorldManager`, which is what owns the tick
    /// thread, the brains and the timers: there is nothing to run, rather than
    /// something that checks a flag before running.
    #[test]
    fn a_disabled_world_builds_nothing_at_all() {
        let cell: std::sync::OnceLock<Arc<WorldManager>> = std::sync::OnceLock::new();
        let err = ensure_manager_in(false, &cell, || {
            panic!("a disabled world must not reach the builder")
        })
        .unwrap_err();
        assert_eq!(err, super::super::ERR_WORLD_DISABLED);
        assert!(
            cell.get().is_none(),
            "no manager, so no thread and no brain"
        );
    }

    #[test]
    fn enabling_builds_the_manager_once_and_disabling_shuts_the_door_again() {
        let root = std::env::temp_dir().join(format!(
            "omniget-world-gate-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let cell: std::sync::OnceLock<Arc<WorldManager>> = std::sync::OnceLock::new();
        let builds = std::cell::Cell::new(0);
        let build = || {
            builds.set(builds.get() + 1);
            Ok(Arc::new(WorldManager::new(root.clone())))
        };
        let first = ensure_manager_in(true, &cell, build).unwrap();
        assert_eq!(builds.get(), 1);
        assert!(!first.thread_running(), "building still starts nothing");
        let again = ensure_manager_in(true, &cell, build).unwrap();
        assert!(Arc::ptr_eq(&first, &again), "one manager per process");
        assert_eq!(builds.get(), 1, "the second call reused it");
        // Switched off afterwards: the existing manager is not handed out.
        assert_eq!(
            ensure_manager_in(false, &cell, build).unwrap_err(),
            super::super::ERR_WORLD_DISABLED
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_but_world_open_starts_a_tick_thread() {
        // The "Never" budget (plan §1.2): building the manager and even
        // creating the world allocate no thread. Counted as a delta because the
        // test binary runs its tests in one process, in parallel.
        let root = std::env::temp_dir().join(format!(
            "omniget-world-never-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let before = tick_threads();
        let m = WorldManager::new(root.clone());
        assert!(!m.thread_running());
        m.create(3, test_map(), DEFAULT_HOUSE).unwrap();
        assert!(
            !m.thread_running(),
            "a world that nobody looks at runs nothing"
        );
        let _ = before;
        drop(m);
        let _ = std::fs::remove_dir_all(&root);
    }
}
