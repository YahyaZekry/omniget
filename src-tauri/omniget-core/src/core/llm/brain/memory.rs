//! Agent memory: the store, the embedding hook and the retrieval score.
//! Owned by f7-world-brain.
//!
//! The smallville retrieval score is three normalised terms added together
//! (`estudos/72` §3.2 — the concept is what we copy, not the Java runtime):
//! recency (exponential decay over game hours), importance (0..10 as the
//! brain rated it) and relevance (cosine similarity against the query
//! embedding). Similarity is brute force in Rust — 10 000 × 384 f32 is a few
//! million multiply-adds, measured in [`bench_retrieve_10k`] — so no SQLite
//! extension is ever loaded.
//!
//! Persistence is `<app_data>/world/memory.sqlite` in WAL mode, with the DDL
//! in `sql/schema.sql`. `rusqlite` is a dependency of the `omniget` app crate
//! but **not** of `omniget-core` yet, so the concrete SQLite store is not
//! compiled here: everything it needs is (schema, statement splitter, blob
//! codec, row shape, the [`MemoryStore`] trait) and [`InMemoryStore`] is the
//! working implementation the tests and the first run use. See the handoff:
//! `cargo.deps` asks for `rusqlite = { version = "0.32", features =
//! ["bundled"] }` on `omniget-core`, at which point the adapter is ~60 lines
//! against this trait.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::schedule::TICKS_PER_GAME_HOUR;
use super::BrainError;

/// Dimensions of the MiniLM-L6-v2 vector `core::embed` produces (f7-embed).
pub const EMBED_DIM: usize = 384;
/// Texts per embedding call. The ONNX session is built for this batch.
pub const EMBED_BATCH: usize = 64;
/// Highest importance the brain assigns. The prompt asks the model for 1..10
/// and the parser clamps to it.
pub const MAX_IMPORTANCE: u8 = 10;

/// Weight of each term of the retrieval score. All three at 1.0 is the
/// original paper's setting; they are named so a future tuning round is a
/// one-line change with a test that fails loudly.
pub const W_RECENCY: f32 = 1.0;
pub const W_IMPORTANCE: f32 = 1.0;
pub const W_RELEVANCE: f32 = 1.0;
/// Recency decays by this factor per game hour: a memory from 24 game hours
/// ago keeps 0.89 of its recency, one from a game week keeps 0.43.
pub const RECENCY_DECAY_PER_HOUR: f32 = 0.995;

/// Pragmas that make the file durable enough and concurrent enough for a tick
/// thread plus a brain task. They return rows, so they go through
/// `execute_batch`, not `execute`.
pub const WAL_PRAGMAS: &str = "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;";

/// The DDL, embedded so the binary carries no data file.
pub const SCHEMA: &str = include_str!("sql/schema.sql");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Observation,
    Reflection,
    Plan,
}

impl MemoryKind {
    /// The integer stored in the `kind` column. Stable: changing it rewrites
    /// everybody's database.
    pub const fn code(self) -> u8 {
        match self {
            MemoryKind::Observation => 0,
            MemoryKind::Reflection => 1,
            MemoryKind::Plan => 2,
        }
    }

    pub const fn from_code(code: u8) -> Option<MemoryKind> {
        match code {
            0 => Some(MemoryKind::Observation),
            1 => Some(MemoryKind::Reflection),
            2 => Some(MemoryKind::Plan),
            _ => None,
        }
    }
}

/// One row of `memory`. `id` is 0 until the store assigns one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: i64,
    pub agent_id: String,
    pub kind: MemoryKind,
    pub text: String,
    pub importance: u8,
    pub embedding: Option<Vec<f32>>,
    pub created_tick: u64,
    pub last_access_tick: u64,
}

impl Memory {
    pub fn new(
        agent_id: impl Into<String>,
        kind: MemoryKind,
        text: impl Into<String>,
        importance: u8,
        created_tick: u64,
    ) -> Memory {
        Memory {
            id: 0,
            agent_id: agent_id.into(),
            kind,
            text: text.into(),
            importance: importance.min(MAX_IMPORTANCE),
            embedding: None,
            created_tick,
            last_access_tick: created_tick,
        }
    }
}

/// A memory picked by [`rank`], with the score that picked it. `index` points
/// back into the slice that was ranked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scored {
    pub index: usize,
    pub score: f32,
    pub recency: f32,
    pub importance: f32,
    pub similarity: f32,
}

/// Cosine similarity. Zero for mismatched or empty vectors instead of NaN:
/// a memory with no embedding must not poison the ranking.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Recency term: `decay ^ (age in game hours)`, clamped to 0..=1.
pub fn recency(created_tick: u64, now_tick: u64) -> f32 {
    let age_ticks = now_tick.saturating_sub(created_tick);
    let hours = age_ticks as f32 / TICKS_PER_GAME_HOUR as f32;
    RECENCY_DECAY_PER_HOUR.powf(hours).clamp(0.0, 1.0)
}

/// Rank memories against a query embedding and return the best `k`, best
/// first. Pure and deterministic: ties break by newer first, then lower id,
/// so two runs on the same data give the same list on every platform.
///
/// An empty `query` means "no semantic question": relevance drops out and the
/// ranking is recency × importance, which is what the reflection prompt wants.
pub fn rank(memories: &[Memory], query: &[f32], now_tick: u64, k: usize) -> Vec<Scored> {
    // A vector of pointers, not of memories: 8 bytes a row instead of 1.5 KB.
    let refs: Vec<&Memory> = memories.iter().collect();
    rank_refs(&refs, query, now_tick, k)
}

/// [`rank`] over borrowed rows, so a store can rank what it holds without
/// first handing out a copy of every embedding it has. The budget line
/// (`retrieve` ≤ 5 ms with 10 000 memories) is about this path: cloning
/// 10 000 × 384 f32 costs more than scoring them does.
pub fn rank_refs(memories: &[&Memory], query: &[f32], now_tick: u64, k: usize) -> Vec<Scored> {
    if k == 0 || memories.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<Scored> = Vec::with_capacity(memories.len());
    for (index, m) in memories.iter().enumerate() {
        let similarity = match (&m.embedding, query.is_empty()) {
            (Some(e), false) => cosine(e, query).max(0.0),
            _ => 0.0,
        };
        let rec = recency(m.created_tick, now_tick);
        let imp = f32::from(m.importance.min(MAX_IMPORTANCE)) / f32::from(MAX_IMPORTANCE);
        scored.push(Scored {
            index,
            score: W_RECENCY * rec + W_IMPORTANCE * imp + W_RELEVANCE * similarity,
            recency: rec,
            importance: imp,
            similarity,
        });
    }
    let k = k.min(scored.len());
    let order = |a: &Scored, b: &Scored| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                memories[b.index]
                    .created_tick
                    .cmp(&memories[a.index].created_tick)
            })
            .then_with(|| memories[a.index].id.cmp(&memories[b.index].id))
            .then_with(|| a.index.cmp(&b.index))
    };
    // Select then sort: the sort is over k, not over the whole corpus.
    if k < scored.len() {
        scored.select_nth_unstable_by(k - 1, order);
        scored.truncate(k);
    }
    scored.sort_unstable_by(order);
    scored
}

/// Split the embedded DDL into executable statements. Comment lines and blank
/// lines drop out; every statement comes back without its trailing `;`.
pub fn schema_statements() -> Vec<String> {
    // Comments go first: a `;` inside one must not split a statement.
    let mut body = String::with_capacity(SCHEMA.len());
    for line in SCHEMA.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }
    let mut out = Vec::new();
    for raw in body.split(';') {
        let stmt = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if !stmt.is_empty() {
            out.push(stmt);
        }
    }
    out
}

/// `<app_data>/world/memory.sqlite`. `None` when the platform has no data
/// directory, in which case the brain stays in memory for the session.
pub fn memory_db_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("OMNIGET_DATA_DIR") {
        return Some(PathBuf::from(dir).join("world").join("memory.sqlite"));
    }
    dirs::data_dir().map(|d| d.join("omniget").join("world").join("memory.sqlite"))
}

/// Encode an embedding the way the `embedding` BLOB column stores it:
/// little-endian f32, 4 bytes per dimension, no header.
pub fn embedding_to_blob(embedding: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(embedding.len() * 4);
    for v in embedding {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Decode an `embedding` BLOB. `None` when the length is not a multiple of 4,
/// which means the row was written by something that is not this codec.
pub fn blob_to_embedding(blob: &[u8]) -> Option<Vec<f32>> {
    if !blob.len().is_multiple_of(4) {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

/// Where memories live. The brain never assumes SQLite: the tick thread, the
/// tests and a first run before the database exists all use
/// [`InMemoryStore`].
pub trait MemoryStore: Send + Sync + std::fmt::Debug {
    /// Store a memory and return the id it was given.
    fn insert(&self, memory: Memory) -> Result<i64, BrainError>;
    /// Every memory of an agent, oldest first.
    fn all_for_agent(&self, agent_id: &str) -> Result<Vec<Memory>, BrainError>;
    /// Attach an embedding to a stored memory.
    fn set_embedding(&self, id: i64, embedding: &[f32]) -> Result<(), BrainError>;
    /// Ids of this agent's memories that still have no embedding, oldest
    /// first, at most `limit`.
    fn unembedded(&self, agent_id: &str, limit: usize) -> Result<Vec<Memory>, BrainError>;
    /// Last game day this agent reflected on; 0 when it never did.
    fn last_reflection_day(&self, agent_id: &str) -> Result<u64, BrainError>;
    fn set_last_reflection_day(&self, agent_id: &str, day: u64) -> Result<(), BrainError>;
    /// Drop the oldest memories of an agent beyond `keep`, so a long session
    /// does not grow without bound. Returns how many were dropped.
    fn prune(&self, agent_id: &str, keep: usize) -> Result<usize, BrainError>;
    fn count(&self, agent_id: &str) -> Result<usize, BrainError>;

    /// The best `k` memories for a query. The default is the honest, slow
    /// way — read everything, rank it — and a store that can rank without
    /// copying its corpus overrides it. See [`retrieve`].
    fn retrieve(
        &self,
        agent_id: &str,
        query_embedding: &[f32],
        now_tick: u64,
        k: usize,
    ) -> Result<Vec<Memory>, BrainError> {
        let all = self.all_for_agent(agent_id)?;
        Ok(rank(&all, query_embedding, now_tick, k)
            .into_iter()
            .map(|s| all[s.index].clone())
            .collect())
    }
}

/// [`MemoryStore::retrieve`] as a free function, so any store gets the same
/// ranking for free: recency × importance × similarity, best `k` first.
pub fn retrieve(
    store: &dyn MemoryStore,
    agent_id: &str,
    query_embedding: &[f32],
    now_tick: u64,
    k: usize,
) -> Result<Vec<Memory>, BrainError> {
    store.retrieve(agent_id, query_embedding, now_tick, k)
}

#[derive(Debug, Default)]
struct InMemoryState {
    next_id: i64,
    rows: BTreeMap<i64, Memory>,
    reflection_day: BTreeMap<String, u64>,
}

/// Memories in a `BTreeMap` keyed by id: deterministic iteration, no
/// dependency, and exactly the semantics the SQLite adapter must reproduce.
#[derive(Debug, Default)]
pub struct InMemoryStore {
    state: Mutex<InMemoryState>,
}

impl InMemoryStore {
    pub fn new() -> InMemoryStore {
        InMemoryStore::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, InMemoryState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl MemoryStore for InMemoryStore {
    fn insert(&self, mut memory: Memory) -> Result<i64, BrainError> {
        let mut st = self.lock();
        st.next_id += 1;
        let id = st.next_id;
        memory.id = id;
        memory.importance = memory.importance.min(MAX_IMPORTANCE);
        st.rows.insert(id, memory);
        Ok(id)
    }

    fn all_for_agent(&self, agent_id: &str) -> Result<Vec<Memory>, BrainError> {
        let st = self.lock();
        Ok(st
            .rows
            .values()
            .filter(|m| m.agent_id == agent_id)
            .cloned()
            .collect())
    }

    fn set_embedding(&self, id: i64, embedding: &[f32]) -> Result<(), BrainError> {
        let mut st = self.lock();
        match st.rows.get_mut(&id) {
            Some(m) => {
                m.embedding = Some(embedding.to_vec());
                Ok(())
            }
            None => Err(BrainError::no_memory(id)),
        }
    }

    fn unembedded(&self, agent_id: &str, limit: usize) -> Result<Vec<Memory>, BrainError> {
        let st = self.lock();
        Ok(st
            .rows
            .values()
            .filter(|m| m.agent_id == agent_id && m.embedding.is_none())
            .take(limit)
            .cloned()
            .collect())
    }

    fn last_reflection_day(&self, agent_id: &str) -> Result<u64, BrainError> {
        Ok(self
            .lock()
            .reflection_day
            .get(agent_id)
            .copied()
            .unwrap_or(0))
    }

    fn set_last_reflection_day(&self, agent_id: &str, day: u64) -> Result<(), BrainError> {
        self.lock().reflection_day.insert(agent_id.to_string(), day);
        Ok(())
    }

    fn prune(&self, agent_id: &str, keep: usize) -> Result<usize, BrainError> {
        let mut st = self.lock();
        let ids: Vec<i64> = st
            .rows
            .values()
            .filter(|m| m.agent_id == agent_id)
            .map(|m| m.id)
            .collect();
        if ids.len() <= keep {
            return Ok(0);
        }
        let drop_count = ids.len() - keep;
        for id in ids.into_iter().take(drop_count) {
            st.rows.remove(&id);
        }
        Ok(drop_count)
    }

    fn count(&self, agent_id: &str) -> Result<usize, BrainError> {
        Ok(self
            .lock()
            .rows
            .values()
            .filter(|m| m.agent_id == agent_id)
            .count())
    }

    /// Ranks under the lock, over references, and clones only the `k` rows it
    /// hands back. The default implementation would copy every embedding in
    /// the corpus first, which is the whole of the 5 ms budget and then some.
    fn retrieve(
        &self,
        agent_id: &str,
        query_embedding: &[f32],
        now_tick: u64,
        k: usize,
    ) -> Result<Vec<Memory>, BrainError> {
        let st = self.lock();
        let mine: Vec<&Memory> = st
            .rows
            .values()
            .filter(|m| m.agent_id == agent_id)
            .collect();
        Ok(rank_refs(&mine, query_embedding, now_tick, k)
            .into_iter()
            .map(|s| mine[s.index].clone())
            .collect())
    }
}

/// The embedding source the brain codes against. `core::embed` (f7-embed)
/// provides the real one; the brain only ever asks for a batch.
#[async_trait]
pub trait Embedder: Send + Sync + std::fmt::Debug {
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, BrainError>;
    fn dim(&self) -> usize {
        EMBED_DIM
    }
}

/// Embed every text in batches of [`EMBED_BATCH`], in order. One call per
/// batch and never one call per text: that is the whole budget rule.
pub async fn embed_all(
    embedder: &dyn Embedder,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, BrainError> {
    let mut out = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(EMBED_BATCH) {
        let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let vectors = embedder.embed_batch(&refs).await?;
        if vectors.len() != refs.len() {
            return Err(BrainError::embed(format!(
                "embedder returned {} vectors for {} texts",
                vectors.len(),
                refs.len()
            )));
        }
        out.extend(vectors);
    }
    Ok(out)
}

/// A deterministic stand-in for the ONNX embedder: hashes whitespace tokens
/// into [`EMBED_DIM`] buckets and L2-normalises. Same text always gives the
/// same vector, similar texts share buckets — enough to test ranking without
/// downloading 23 MB of model, and never used outside tests and `--ignored`
/// benches.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeEmbedder;

impl FakeEmbedder {
    pub fn vector(text: &str) -> Vec<f32> {
        let mut v = vec![0.0f32; EMBED_DIM];
        for token in text.split_whitespace() {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for b in token.to_lowercase().bytes() {
                hash ^= u64::from(b);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            let bucket = (hash % EMBED_DIM as u64) as usize;
            v[bucket] += 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}

#[async_trait]
impl Embedder for FakeEmbedder {
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, BrainError> {
        Ok(texts.iter().map(|t| FakeEmbedder::vector(t)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(id: i64, text: &str, importance: u8, tick: u64) -> Memory {
        let mut m = Memory::new("a", MemoryKind::Observation, text, importance, tick);
        m.id = id;
        m.embedding = Some(FakeEmbedder::vector(text));
        m
    }

    #[test]
    fn cosine_is_one_for_the_same_vector_and_zero_for_mismatched_lengths() {
        let v = FakeEmbedder::vector("the workbench is busy");
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-5);
        assert_eq!(cosine(&v, &[]), 0.0);
        assert_eq!(cosine(&v, &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[0.0, 0.0]), 0.0);
    }

    #[test]
    fn recency_decays_with_game_hours() {
        assert!((recency(100, 100) - 1.0).abs() < 1e-6);
        let one_hour = recency(0, TICKS_PER_GAME_HOUR);
        let one_day = recency(0, TICKS_PER_GAME_HOUR * 24);
        assert!(
            (one_hour - RECENCY_DECAY_PER_HOUR).abs() < 1e-4,
            "{one_hour}"
        );
        assert!(one_day < one_hour);
        assert!(one_day > 0.8, "{one_day}");
        // A tick in the future must not score above a fresh memory.
        assert!(recency(200, 100) <= 1.0);
    }

    #[test]
    fn rank_puts_the_relevant_memory_first() {
        let memories = vec![
            mem(1, "i watered the plants in the kitchen", 3, 0),
            mem(2, "the workbench needs a new saw", 5, 0),
            mem(3, "the cat slept on the sofa", 2, 0),
        ];
        let query = FakeEmbedder::vector("workbench saw");
        let top = rank(&memories, &query, 100, 1);
        assert_eq!(top.len(), 1);
        assert_eq!(memories[top[0].index].id, 2);
        assert!(top[0].similarity > 0.0);
    }

    #[test]
    fn rank_prefers_the_important_and_recent_when_relevance_ties() {
        let memories = vec![
            mem(1, "alpha", 1, 0),
            mem(2, "alpha", 9, 0),
            mem(3, "alpha", 1, TICKS_PER_GAME_HOUR * 100),
        ];
        let top = rank(&memories, &[], TICKS_PER_GAME_HOUR * 100, 3);
        assert_eq!(memories[top[0].index].id, 2, "importance wins");
        assert_eq!(memories[top[1].index].id, 3, "recency wins next");
    }

    #[test]
    fn rank_is_deterministic_on_a_full_tie() {
        let memories: Vec<Memory> = (1..=5).map(|i| mem(i, "same text", 4, 10)).collect();
        let first = rank(&memories, &[], 10, 3);
        let second = rank(&memories, &[], 10, 3);
        assert_eq!(
            first.iter().map(|s| s.index).collect::<Vec<_>>(),
            second.iter().map(|s| s.index).collect::<Vec<_>>()
        );
        assert_eq!(memories[first[0].index].id, 1);
    }

    #[test]
    fn rank_handles_k_larger_than_the_corpus_and_zero_k() {
        let memories = vec![mem(1, "only one", 5, 0)];
        assert_eq!(rank(&memories, &[], 0, 10).len(), 1);
        assert!(rank(&memories, &[], 0, 0).is_empty());
        assert!(rank(&[], &[], 0, 5).is_empty());
    }

    #[test]
    fn a_memory_without_an_embedding_still_ranks_on_recency_and_importance() {
        let mut naked = Memory::new("a", MemoryKind::Observation, "no vector", 10, 0);
        naked.id = 7;
        let memories = vec![naked, mem(8, "with vector", 1, 0)];
        let top = rank(&memories, &FakeEmbedder::vector("with vector"), 0, 2);
        assert_eq!(memories[top[0].index].id, 8, "relevance wins here");
        assert_eq!(
            memories[top[1].index].id, 7,
            "but the naked one still ranks"
        );
        assert_eq!(top[1].similarity, 0.0);
        // With no question asked, importance decides and the naked one wins.
        let no_query = rank(&memories, &[], 0, 1);
        assert_eq!(memories[no_query[0].index].id, 7);
    }

    #[test]
    fn the_blob_codec_round_trips() {
        let v = FakeEmbedder::vector("round trip");
        let blob = embedding_to_blob(&v);
        assert_eq!(blob.len(), EMBED_DIM * 4);
        assert_eq!(blob_to_embedding(&blob).unwrap(), v);
        assert_eq!(blob_to_embedding(&[]), Some(Vec::new()));
        assert_eq!(blob_to_embedding(&[1, 2, 3]), None);
    }

    #[test]
    fn the_schema_splits_into_executable_statements() {
        let stmts = schema_statements();
        assert_eq!(stmts.len(), 5, "{stmts:#?}");
        assert!(stmts.iter().all(|s| !s.contains("--")));
        assert!(stmts.iter().all(|s| !s.trim().is_empty()));
        assert!(stmts[0].starts_with("CREATE TABLE IF NOT EXISTS memory"));
        assert!(stmts.iter().any(|s| s.contains("brain_state")));
        assert!(
            !stmts.iter().any(|s| s.contains("PRAGMA")),
            "pragmas return rows and never go through execute()"
        );
        assert!(WAL_PRAGMAS.contains("journal_mode=WAL"));
    }

    #[test]
    fn the_kind_codes_are_stable() {
        for kind in [
            MemoryKind::Observation,
            MemoryKind::Reflection,
            MemoryKind::Plan,
        ] {
            assert_eq!(MemoryKind::from_code(kind.code()), Some(kind));
        }
        assert_eq!(MemoryKind::Observation.code(), 0);
        assert_eq!(MemoryKind::from_code(9), None);
    }

    #[test]
    fn the_in_memory_store_inserts_embeds_prunes_and_counts() {
        let store = InMemoryStore::new();
        for i in 0..5 {
            store
                .insert(Memory::new(
                    "a",
                    MemoryKind::Observation,
                    format!("event {i}"),
                    3,
                    i,
                ))
                .unwrap();
        }
        store
            .insert(Memory::new("b", MemoryKind::Observation, "other", 3, 0))
            .unwrap();
        assert_eq!(store.count("a").unwrap(), 5);
        assert_eq!(store.count("b").unwrap(), 1);

        let pending = store.unembedded("a", 3).unwrap();
        assert_eq!(pending.len(), 3);
        store.set_embedding(pending[0].id, &[0.5; 4]).unwrap();
        assert_eq!(store.unembedded("a", 10).unwrap().len(), 4);
        assert!(store.set_embedding(9999, &[0.1]).is_err());

        assert_eq!(store.prune("a", 2).unwrap(), 3);
        assert_eq!(store.count("a").unwrap(), 2);
        assert_eq!(store.count("b").unwrap(), 1, "other agents untouched");
        assert_eq!(store.prune("a", 10).unwrap(), 0);
    }

    #[test]
    fn importance_is_clamped_on_the_way_in() {
        let store = InMemoryStore::new();
        let id = store
            .insert(Memory::new("a", MemoryKind::Plan, "way too loud", 200, 0))
            .unwrap();
        let stored = store.all_for_agent("a").unwrap();
        assert_eq!(stored[0].id, id);
        assert_eq!(stored[0].importance, MAX_IMPORTANCE);
    }

    #[test]
    fn the_reflection_day_survives_a_round_trip() {
        let store = InMemoryStore::new();
        assert_eq!(store.last_reflection_day("a").unwrap(), 0);
        store.set_last_reflection_day("a", 4).unwrap();
        assert_eq!(store.last_reflection_day("a").unwrap(), 4);
        assert_eq!(store.last_reflection_day("b").unwrap(), 0);
    }

    #[test]
    fn retrieve_reads_the_store_and_ranks() {
        let store = InMemoryStore::new();
        for (text, importance) in [("the sofa is soft", 2), ("the workbench is hot", 8)] {
            let mut m = Memory::new("a", MemoryKind::Observation, text, importance, 0);
            m.embedding = Some(FakeEmbedder::vector(text));
            store.insert(m).unwrap();
        }
        let query = FakeEmbedder::vector("workbench");
        let got = retrieve(&store, "a", &query, 10, 1).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "the workbench is hot");
    }

    #[tokio::test]
    async fn embed_all_batches_by_64_and_keeps_the_order() {
        #[derive(Debug, Default)]
        struct Counting {
            calls: std::sync::atomic::AtomicUsize,
        }
        #[async_trait]
        impl Embedder for Counting {
            async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, BrainError> {
                assert!(texts.len() <= EMBED_BATCH);
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(texts.iter().map(|t| FakeEmbedder::vector(t)).collect())
            }
        }
        let counting = Counting::default();
        let texts: Vec<String> = (0..130).map(|i| format!("text {i}")).collect();
        let vectors = embed_all(&counting, &texts).await.unwrap();
        assert_eq!(vectors.len(), 130);
        assert_eq!(counting.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
        assert_eq!(vectors[0], FakeEmbedder::vector("text 0"));
        assert_eq!(vectors[129], FakeEmbedder::vector("text 129"));
    }

    #[tokio::test]
    async fn embed_all_refuses_a_short_answer() {
        #[derive(Debug)]
        struct Short;
        #[async_trait]
        impl Embedder for Short {
            async fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, BrainError> {
                Ok(vec![vec![0.0; EMBED_DIM]])
            }
        }
        let texts: Vec<String> = (0..3).map(|i| format!("t{i}")).collect();
        let err = embed_all(&Short, &texts).await.unwrap_err();
        assert_eq!(err.code, super::super::ERR_BRAIN_EMBED);
    }

    /// Budget check from the prompt §5: `retrieve` under 5 ms with 10 000
    /// memories of 384 dimensions. Ignored because it is a measurement, not a
    /// contract; run it with
    /// `cargo test --release -p omniget-core --features desktop
    ///  llm::brain::memory::tests::bench_retrieve_10k -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement; run in release"]
    fn bench_retrieve_10k() {
        let store = InMemoryStore::new();
        for i in 0..10_000u64 {
            let text = format!("memory number {i} about the workbench and the kitchen");
            let mut m = Memory::new("a", MemoryKind::Observation, &text, (i % 11) as u8, i);
            m.embedding = Some(FakeEmbedder::vector(&text));
            store.insert(m).unwrap();
        }
        let query = FakeEmbedder::vector("what happened at the workbench today");
        // Warm up the clone of the corpus so the number is the ranking cost.
        let _ = retrieve(&store, "a", &query, 10_000, 8).unwrap();
        let rounds = 20;
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            let got = retrieve(&store, "a", &query, 10_000, 8).unwrap();
            assert_eq!(got.len(), 8);
        }
        let per = start.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds);
        // The ranking alone, without the store's clone of 10 000 rows.
        let all = store.all_for_agent("a").unwrap();
        let start = std::time::Instant::now();
        for _ in 0..rounds {
            let _ = rank(&all, &query, 10_000, 8);
        }
        let rank_only = start.elapsed().as_secs_f64() * 1000.0 / f64::from(rounds);
        println!(
            "retrieve 10000 memories x 384 dims: {per:.3} ms per call (rank only {rank_only:.3} ms), {rounds} rounds"
        );
    }
}
