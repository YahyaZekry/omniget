//! `<app_data>/world/memory.sqlite`: the brain's memories on disk. Owned by
//! f7-world-bridge / m4-world.
//!
//! `core::llm::brain::memory` owns everything about the data — the DDL
//! (`sql/schema.sql`), the statement splitter, the WAL pragmas, the little-
//! endian blob codec for the embedding, the row shape and the ranking. What it
//! does not own is a SQLite driver: `rusqlite` is a dependency of the `omniget`
//! app crate and not of `omniget-core`, so the concrete store lives here,
//! against the [`MemoryStore`] trait, and is the ~60 lines the brain's module
//! doc predicted.
//!
//! If `rusqlite = { version = "0.32", features = ["bundled"] }` is ever added to
//! `omniget-core/Cargo.toml`, this file moves into `brain/memory.rs` unchanged;
//! nothing here knows it is in the app crate. Until then the app is the only
//! crate that links SQLite, which is also what keeps `omniget-core`'s test suite
//! free of a C toolchain.
//!
//! Concurrency: WAL plus `synchronous=NORMAL`, one connection behind a mutex.
//! The tick thread and a brain task both write, and both are short; a pool
//! would buy nothing at ten writes a second.

use std::path::Path;
use std::sync::{Arc, Mutex};

use omniget_core::core::llm::brain::memory::{
    blob_to_embedding, embedding_to_blob, schema_statements, Memory, MemoryKind, MemoryStore,
    MAX_IMPORTANCE, WAL_PRAGMAS,
};
use omniget_core::core::llm::brain::BrainError;
use rusqlite::{params, Connection, OptionalExtension};

/// `<world root>/memory.sqlite`, beside `save.bin`, so that deleting the world
/// deletes the memories with it.
pub fn memory_path(root: &Path) -> std::path::PathBuf {
    super::save::world_dir(root).join("memory.sqlite")
}

/// Memories in SQLite.
pub struct SqliteMemoryStore {
    conn: Mutex<Connection>,
}

impl std::fmt::Debug for SqliteMemoryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteMemoryStore").finish()
    }
}

fn store_err(e: impl std::fmt::Display) -> BrainError {
    BrainError::store(e.to_string())
}

impl SqliteMemoryStore {
    /// Open (and create) the database, applying the DDL every time: every
    /// statement in the schema is `IF NOT EXISTS`, so this is the migration.
    pub fn open(path: &Path) -> Result<SqliteMemoryStore, BrainError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(store_err)?;
        }
        let conn = Connection::open(path).map_err(store_err)?;
        Self::prepare(conn)
    }

    /// An in-memory database, for tests: same driver, same DDL, no file.
    pub fn open_in_memory() -> Result<SqliteMemoryStore, BrainError> {
        Self::prepare(Connection::open_in_memory().map_err(store_err)?)
    }

    fn prepare(conn: Connection) -> Result<SqliteMemoryStore, BrainError> {
        // The pragmas return rows, so they cannot go through `execute`.
        conn.execute_batch(WAL_PRAGMAS).map_err(store_err)?;
        for statement in schema_statements() {
            conn.execute(&statement, []).map_err(store_err)?;
        }
        Ok(SqliteMemoryStore {
            conn: Mutex::new(conn),
        })
    }

    /// The store the brains of a house share, or [`None`] when SQLite will not
    /// open — a read-only home directory is a reason to keep the memories in
    /// RAM for the session, never a reason to refuse to run the world.
    pub fn shared(root: &Path) -> Option<Arc<dyn MemoryStore>> {
        match SqliteMemoryStore::open(&memory_path(root)) {
            Ok(store) => Some(Arc::new(store)),
            Err(e) => {
                tracing::warn!("[world] brain memory falls back to RAM: {e}");
                None
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Memory> {
        let kind: u8 = r.get::<_, i64>(2)? as u8;
        let blob: Option<Vec<u8>> = r.get(7)?;
        Ok(Memory {
            id: r.get(0)?,
            agent_id: r.get(1)?,
            kind: MemoryKind::from_code(kind).unwrap_or(MemoryKind::Observation),
            text: r.get(3)?,
            importance: r.get::<_, i64>(4)? as u8,
            created_tick: r.get::<_, i64>(5)? as u64,
            last_access_tick: r.get::<_, i64>(6)? as u64,
            embedding: blob.as_deref().and_then(blob_to_embedding),
        })
    }

    fn select(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Memory>, BrainError> {
        let conn = self.lock();
        let mut stmt = conn.prepare(sql).map_err(store_err)?;
        let rows = stmt
            .query_map(args, SqliteMemoryStore::row)
            .map_err(store_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(store_err)?);
        }
        Ok(out)
    }
}

const COLUMNS: &str =
    "id, agent_id, kind, text, importance, created_tick, last_access_tick, embedding";

impl MemoryStore for SqliteMemoryStore {
    fn insert(&self, memory: Memory) -> Result<i64, BrainError> {
        let conn = self.lock();
        let blob = memory.embedding.as_deref().map(embedding_to_blob);
        conn.execute(
            "INSERT INTO memory (agent_id, kind, text, importance, created_tick, \
             last_access_tick, embedding) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                memory.agent_id,
                i64::from(memory.kind.code()),
                memory.text,
                i64::from(memory.importance.min(MAX_IMPORTANCE)),
                memory.created_tick as i64,
                memory.last_access_tick as i64,
                blob,
            ],
        )
        .map_err(store_err)?;
        Ok(conn.last_insert_rowid())
    }

    fn all_for_agent(&self, agent_id: &str) -> Result<Vec<Memory>, BrainError> {
        self.select(
            &format!("SELECT {COLUMNS} FROM memory WHERE agent_id = ?1 ORDER BY id ASC"),
            &[&agent_id],
        )
    }

    fn set_embedding(&self, id: i64, embedding: &[f32]) -> Result<(), BrainError> {
        let conn = self.lock();
        let changed = conn
            .execute(
                "UPDATE memory SET embedding = ?2 WHERE id = ?1",
                params![id, embedding_to_blob(embedding)],
            )
            .map_err(store_err)?;
        if changed == 0 {
            return Err(BrainError::no_memory(id));
        }
        Ok(())
    }

    fn unembedded(&self, agent_id: &str, limit: usize) -> Result<Vec<Memory>, BrainError> {
        self.select(
            &format!(
                "SELECT {COLUMNS} FROM memory WHERE agent_id = ?1 AND embedding IS NULL \
                 ORDER BY id ASC LIMIT ?2"
            ),
            &[&agent_id, &(limit as i64)],
        )
    }

    fn last_reflection_day(&self, agent_id: &str) -> Result<u64, BrainError> {
        let conn = self.lock();
        let day: Option<i64> = conn
            .query_row(
                "SELECT last_reflection_day FROM brain_state WHERE agent_id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(store_err)?;
        Ok(day.unwrap_or(0).max(0) as u64)
    }

    fn set_last_reflection_day(&self, agent_id: &str, day: u64) -> Result<(), BrainError> {
        self.lock()
            .execute(
                "INSERT INTO brain_state (agent_id, last_reflection_day) VALUES (?1, ?2) \
                 ON CONFLICT(agent_id) DO UPDATE SET last_reflection_day = excluded.last_reflection_day",
                params![agent_id, day as i64],
            )
            .map_err(store_err)?;
        Ok(())
    }

    fn prune(&self, agent_id: &str, keep: usize) -> Result<usize, BrainError> {
        // Oldest first is by id, not by tick: a catch-up can write a memory
        // whose game time is older than one already stored, and "what I wrote
        // first" is the order `InMemoryStore` drops in too.
        let dropped = self
            .lock()
            .execute(
                "DELETE FROM memory WHERE agent_id = ?1 AND id NOT IN \
                 (SELECT id FROM memory WHERE agent_id = ?1 ORDER BY id DESC LIMIT ?2)",
                params![agent_id, keep as i64],
            )
            .map_err(store_err)?;
        Ok(dropped)
    }

    fn count(&self, agent_id: &str) -> Result<usize, BrainError> {
        let conn = self.lock();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM memory WHERE agent_id = ?1",
                params![agent_id],
                |r| r.get(0),
            )
            .map_err(store_err)?;
        Ok(n.max(0) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omniget_core::core::llm::brain::memory::{retrieve, FakeEmbedder, InMemoryStore};

    fn observation(agent: &str, text: &str, importance: u8, tick: u64) -> Memory {
        let mut m = Memory::new(agent, MemoryKind::Observation, text, importance, tick);
        m.embedding = Some(FakeEmbedder::vector(text));
        m
    }

    #[test]
    fn the_schema_applies_and_re_applies() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        // Every statement is IF NOT EXISTS, so opening an existing database is
        // the whole migration story.
        let conn = store.lock();
        for statement in schema_statements() {
            conn.execute(&statement, []).expect("idempotent DDL");
        }
    }

    #[test]
    fn a_memory_round_trips_with_its_embedding() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let id = store
            .insert(observation("ada", "the radio is broken", 7, 42))
            .unwrap();
        assert!(id > 0);
        let got = store.all_for_agent("ada").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, id);
        assert_eq!(got[0].text, "the radio is broken");
        assert_eq!(got[0].importance, 7);
        assert_eq!(got[0].created_tick, 42);
        assert_eq!(got[0].kind, MemoryKind::Observation);
        let embedding = got[0].embedding.as_ref().expect("the blob came back");
        assert_eq!(embedding.len(), 384);
        assert_eq!(embedding, &FakeEmbedder::vector("the radio is broken"));
        // Another agent's rows are not this agent's.
        assert!(store.all_for_agent("grace").unwrap().is_empty());
        assert_eq!(store.count("ada").unwrap(), 1);
    }

    #[test]
    fn an_embedding_can_be_attached_later() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        let id = store
            .insert(Memory::new("ada", MemoryKind::Observation, "later", 1, 1))
            .unwrap();
        assert_eq!(store.unembedded("ada", 10).unwrap().len(), 1);
        store
            .set_embedding(id, &FakeEmbedder::vector("later"))
            .unwrap();
        assert!(store.unembedded("ada", 10).unwrap().is_empty());
        assert!(store.all_for_agent("ada").unwrap()[0].embedding.is_some());
        assert_eq!(
            store.set_embedding(9_999, &[0.0]).unwrap_err().code,
            omniget_core::core::llm::brain::ERR_BRAIN_STORE
        );
    }

    #[test]
    fn the_reflection_day_is_remembered_per_agent() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        assert_eq!(store.last_reflection_day("ada").unwrap(), 0);
        store.set_last_reflection_day("ada", 3).unwrap();
        store.set_last_reflection_day("ada", 4).unwrap();
        assert_eq!(store.last_reflection_day("ada").unwrap(), 4);
        assert_eq!(store.last_reflection_day("grace").unwrap(), 0);
    }

    #[test]
    fn pruning_keeps_the_newest_and_leaves_the_neighbours_alone() {
        let store = SqliteMemoryStore::open_in_memory().unwrap();
        for i in 0..10u64 {
            store
                .insert(observation("ada", &format!("thing {i}"), 1, i))
                .unwrap();
        }
        store.insert(observation("grace", "hers", 1, 0)).unwrap();
        assert_eq!(store.prune("ada", 4).unwrap(), 6);
        let left = store.all_for_agent("ada").unwrap();
        assert_eq!(left.len(), 4);
        assert_eq!(left[0].text, "thing 6", "the newest four survived");
        assert_eq!(store.count("grace").unwrap(), 1);
        // Nothing to do is not an error.
        assert_eq!(store.prune("ada", 50).unwrap(), 0);
    }

    /// The whole point of the trait: two stores, one ranking. Same rows in,
    /// same order out.
    #[test]
    fn sqlite_and_the_in_memory_store_rank_identically() {
        let sqlite = SqliteMemoryStore::open_in_memory().unwrap();
        let ram = InMemoryStore::new();
        let rows = [
            ("the radio is broken", 7u8, 10u64),
            ("the cat is on the sofa", 2, 20),
            ("Grace fixed the radio at the workbench", 5, 30),
            ("it rained", 1, 40),
        ];
        for (text, importance, tick) in rows {
            let m = observation("ada", text, importance, tick);
            sqlite.insert(m.clone()).unwrap();
            ram.insert(m).unwrap();
        }
        let query = FakeEmbedder::vector("what happened to the radio");
        let a = retrieve(&sqlite, "ada", &query, 50, 3).unwrap();
        let b = retrieve(&ram, "ada", &query, 50, 3).unwrap();
        assert_eq!(a.len(), 3);
        let texts = |v: &[Memory]| v.iter().map(|m| m.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&a), texts(&b));
        assert!(a[0].text.contains("radio"), "{:?}", a[0].text);
    }

    #[test]
    fn the_memories_survive_closing_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "omniget-brain-db-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let path = memory_path(&dir);
        {
            let store = SqliteMemoryStore::open(&path).unwrap();
            store
                .insert(observation("ada", "remember me", 9, 1))
                .unwrap();
            store.set_last_reflection_day("ada", 2).unwrap();
        }
        let reopened = SqliteMemoryStore::open(&path).unwrap();
        assert_eq!(reopened.count("ada").unwrap(), 1);
        assert_eq!(reopened.last_reflection_day("ada").unwrap(), 2);
        assert_eq!(
            reopened.all_for_agent("ada").unwrap()[0].text,
            "remember me"
        );
        assert!(
            path.starts_with(super::super::save::world_dir(&dir)),
            "the database lives under <app_data>/world/, so deleting the world deletes it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
