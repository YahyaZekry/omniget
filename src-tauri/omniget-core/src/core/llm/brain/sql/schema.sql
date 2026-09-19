-- Agent memory for the world brain (core/llm/brain). Owned by f7-world-brain.
--
-- DDL only, one statement per `;`, no `;` inside a statement: it is applied
-- statement by statement by `memory::schema_statements()`. The WAL and
-- synchronous pragmas are NOT here because they return rows; they live in
-- `memory::WAL_PRAGMAS` and go through `execute_batch`.
--
-- `embedding` is little-endian f32, 4 bytes per dimension, NULL until the
-- embedder has run (see `memory::embedding_to_blob`). Similarity is computed
-- in Rust: no SQLite extension is loaded, ever.

CREATE TABLE IF NOT EXISTS memory (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  agent_id         TEXT    NOT NULL,
  kind             INTEGER NOT NULL,
  text             TEXT    NOT NULL,
  importance       INTEGER NOT NULL,
  created_tick     INTEGER NOT NULL,
  last_access_tick INTEGER NOT NULL,
  embedding        BLOB
);

CREATE INDEX IF NOT EXISTS memory_agent_tick ON memory (agent_id, created_tick DESC);

CREATE INDEX IF NOT EXISTS memory_agent_kind ON memory (agent_id, kind, created_tick DESC);

CREATE INDEX IF NOT EXISTS memory_unembedded ON memory (agent_id) WHERE embedding IS NULL;

CREATE TABLE IF NOT EXISTS brain_state (
  agent_id            TEXT    PRIMARY KEY,
  last_reflection_day INTEGER NOT NULL DEFAULT 0,
  last_plan_day       INTEGER NOT NULL DEFAULT 0,
  schedule_json       TEXT
);
