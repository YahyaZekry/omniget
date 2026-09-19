//! Ledger local de uso de IA (estudo 17, ccusage): cada chamada a LLM/TTS
//! vira uma linha em `<app_data>/tools/ai_usage.jsonl`. O custo é calculado
//! na leitura, com a tabela de preços do momento, para poder recalcular.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageEntry {
    pub ts: String,
    /// Texto livre: "chat", "grok", "transcribe", "humanize", ou o que o agente
    /// quiser. A UI agrupa por ele sem tabela fechada.
    pub task: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub characters: u64,
    #[serde(default)]
    pub seconds: f64,
    /// Custo informado na hora (ex.: 0 para Ollama). `None` = calcular.
    #[serde(default)]
    pub cost_usd: Option<f64>,
    // ── Campos da expansao LLM (f2-llm-providers). Todos com `default`, e por
    // isso as linhas antigas do JSONL continuam sendo lidas sem migracao.
    /// Agente dono da chamada (`AgentDef::id`); vazio quando veio do app.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub agent_id: String,
    /// Tokens de entrada que o provedor serviu do cache de prompt.
    #[serde(default)]
    pub cache_read_tokens: u64,
    /// Tokens gravados no cache de prompt (Anthropic cobra a mais por eles).
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// Latencia ate o primeiro token, em ms (so no streaming).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u32>,
}

impl UsageEntry {
    /// Entrada carimbada com o instante atual; os campos de numero ficam em
    /// zero para quem so tem alguns deles.
    pub fn now(task: &str, provider: &str, model: &str) -> Self {
        Self {
            ts: chrono::Utc::now().to_rfc3339(),
            task: task.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            ..Self::default()
        }
    }
}

fn path() -> Option<PathBuf> {
    super::tools_dir().map(|d| d.join("ai_usage.jsonl"))
}

/// Grava sem bloquear quem chamou; erro de disco vira só um log.
pub fn record(
    task: &str,
    provider: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: Option<f64>,
) {
    record_entry(UsageEntry {
        input_tokens,
        output_tokens,
        cost_usd,
        ..UsageEntry::now(task, provider, model)
    });
}

/// Grava uma linha inteira (com agente, cache e latencia). Mesma politica do
/// `record`: erro de disco vira log, nunca erro para quem chamou.
pub fn record_entry(mut entry: UsageEntry) {
    if entry.ts.is_empty() {
        entry.ts = chrono::Utc::now().to_rfc3339();
    }
    let Some(p) = path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(line) = serde_json::to_string(&entry) else {
        return;
    };
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p)
    {
        Ok(mut f) => {
            let _ = writeln!(f, "{}", line);
        }
        Err(e) => tracing::warn!("[usage] nao gravou: {}", e),
    }
}

pub fn read_all() -> Vec<UsageEntry> {
    let Some(p) = path() else { return vec![] };
    let Ok(text) = std::fs::read_to_string(&p) else {
        return vec![];
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn clear() -> anyhow::Result<()> {
    if let Some(p) = path() {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Bucket {
    pub key: String,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
    pub unknown_price: u64,
}

/// Custo de uma linha com a tabela de precos, levando o cache em conta: token
/// servido do cache sai do preco de entrada e entra pelo preco de leitura de
/// cache; token gravado no cache entra pelo preco de escrita. Sem esses precos
/// na tabela, tudo volta a valer o preco de entrada (o que o `pricing::cost`
/// ja fazia).
///
/// Convencao do `UsageEntry`: `input_tokens` e o total de entrada, cache
/// incluso, como o `prompt_tokens` da OpenAI. Os clientes normalizam a
/// Anthropic (que reporta os tres separados) para essa mesma forma.
pub fn cost_of(price: &super::pricing::ModelPrice, e: &UsageEntry) -> Option<f64> {
    let in_per = price.input_per_m? / 1_000_000.0;
    let out_per = price.output_per_m.unwrap_or(0.0) / 1_000_000.0;
    let read_per = price.cache_read_per_m.map(|v| v / 1e6).unwrap_or(in_per);
    let write_per = price.cache_write_per_m.map(|v| v / 1e6).unwrap_or(in_per);
    let cached = e.cache_read_tokens.min(e.input_tokens);
    let written = e.cache_write_tokens.min(e.input_tokens - cached);
    let fresh = e.input_tokens - cached - written;
    Some(
        fresh as f64 * in_per
            + cached as f64 * read_per
            + written as f64 * write_per
            + e.output_tokens as f64 * out_per,
    )
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageReport {
    pub since: String,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub unknown_price: u64,
    pub by_day: Vec<Bucket>,
    pub by_model: Vec<Bucket>,
    pub by_task: Vec<Bucket>,
    /// Chave = `agent_id`; linhas sem agente caem em "app".
    pub by_agent: Vec<Bucket>,
    pub entries_path: Option<String>,
}

pub async fn report(days: u32) -> UsageReport {
    let since = chrono::Utc::now() - chrono::Duration::days(days.max(1) as i64);
    let entries: Vec<UsageEntry> = read_all()
        .into_iter()
        .filter(|e| {
            chrono::DateTime::parse_from_rfc3339(&e.ts)
                .map(|t| t.with_timezone(&chrono::Utc) >= since)
                .unwrap_or(false)
        })
        .collect();
    let mut by_day: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_model: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_task: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_agent: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut total = Bucket::default();
    let mut price_cache: std::collections::HashMap<String, Option<super::pricing::ModelPrice>> =
        Default::default();
    for e in &entries {
        let cost = match e.cost_usd {
            Some(c) => Some(c),
            None => {
                let p = match price_cache.get(&e.model) {
                    Some(p) => p.clone(),
                    None => {
                        let p = super::pricing::price_for(&e.model).await;
                        price_cache.insert(e.model.clone(), p.clone());
                        p
                    }
                };
                p.and_then(|p| cost_of(&p, e))
            }
        };
        let day = e.ts.get(..10).unwrap_or("").to_string();
        let agent = if e.agent_id.is_empty() {
            "app".to_string()
        } else {
            e.agent_id.clone()
        };
        for (map, key) in [
            (&mut by_day, day),
            (&mut by_model, e.model.clone()),
            (&mut by_task, e.task.clone()),
            (&mut by_agent, agent),
        ] {
            let b = map.entry(key.clone()).or_insert_with(|| Bucket {
                key,
                ..Default::default()
            });
            b.calls += 1;
            b.input_tokens += e.input_tokens;
            b.output_tokens += e.output_tokens;
            b.cache_read_tokens += e.cache_read_tokens;
            b.cache_write_tokens += e.cache_write_tokens;
            match cost {
                Some(c) => b.cost_usd += c,
                None => b.unknown_price += 1,
            }
        }
        total.calls += 1;
        total.input_tokens += e.input_tokens;
        total.output_tokens += e.output_tokens;
        match cost {
            Some(c) => total.cost_usd += c,
            None => total.unknown_price += 1,
        }
    }
    let mut by_model: Vec<Bucket> = by_model.into_values().collect();
    by_model.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.calls.cmp(&a.calls))
    });
    UsageReport {
        since: since.to_rfc3339(),
        calls: total.calls,
        input_tokens: total.input_tokens,
        output_tokens: total.output_tokens,
        cost_usd: total.cost_usd,
        unknown_price: total.unknown_price,
        by_day: by_day.into_values().collect(),
        by_model,
        by_task: by_task.into_values().collect(),
        by_agent: by_agent.into_values().collect(),
        entries_path: path().map(|p| p.to_string_lossy().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Linha escrita antes da expansao LLM: sem agente, sem cache, sem
    /// latencia. Precisa continuar sendo lida.
    #[test]
    fn reads_a_pre_expansion_line() {
        let old = r#"{"ts":"2026-01-02T03:04:05Z","task":"chat","provider":"openai","model":"gpt-4o-mini","input_tokens":10,"output_tokens":20,"characters":0,"seconds":0.0,"cost_usd":null}"#;
        let e: UsageEntry = serde_json::from_str(old).expect("linha antiga");
        assert_eq!(e.input_tokens, 10);
        assert_eq!(e.agent_id, "");
        assert_eq!(e.cache_read_tokens, 0);
        assert_eq!(e.first_token_ms, None);
    }

    #[test]
    fn new_fields_survive_a_round_trip() {
        let e = UsageEntry {
            input_tokens: 100,
            output_tokens: 7,
            cache_read_tokens: 64,
            cache_write_tokens: 8,
            first_token_ms: Some(312),
            agent_id: "coordinator".into(),
            ..UsageEntry::now("chat", "anthropic", "claude-sonnet-4-5")
        };
        let line = serde_json::to_string(&e).unwrap();
        let back: UsageEntry = serde_json::from_str(&line).unwrap();
        assert_eq!(back.agent_id, "coordinator");
        assert_eq!(back.cache_read_tokens, 64);
        assert_eq!(back.first_token_ms, Some(312));
        assert!(!back.ts.is_empty());
    }

    /// Campos vazios nao sujam a linha: o JSONL de quem nao usa agente fica
    /// igual ao de antes, menos os dois contadores de cache.
    #[test]
    fn empty_agent_and_latency_are_not_written() {
        let line = serde_json::to_string(&UsageEntry::now("chat", "openai", "m")).unwrap();
        assert!(!line.contains("agent_id"));
        assert!(!line.contains("first_token_ms"));
    }

    fn price() -> super::super::pricing::ModelPrice {
        super::super::pricing::ModelPrice {
            key: "m".into(),
            provider: "anthropic".into(),
            mode: "chat".into(),
            input_per_m: Some(3.0),
            output_per_m: Some(15.0),
            cache_read_per_m: Some(0.3),
            cache_write_per_m: Some(3.75),
            max_input_tokens: None,
            max_output_tokens: None,
            input_per_second: None,
            input_per_character: None,
            supports_vision: false,
            supports_tools: true,
            supports_reasoning: false,
            supports_caching: true,
            deprecation_date: None,
        }
    }

    #[test]
    fn cost_without_cache_matches_the_plain_formula() {
        let e = UsageEntry {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..UsageEntry::now("chat", "anthropic", "m")
        };
        let c = cost_of(&price(), &e).unwrap();
        assert!((c - 18.0).abs() < 1e-9, "{}", c);
    }

    #[test]
    fn cached_tokens_are_billed_at_the_cache_price() {
        let e = UsageEntry {
            input_tokens: 1_000_000,
            cache_read_tokens: 1_000_000,
            output_tokens: 0,
            ..UsageEntry::now("chat", "anthropic", "m")
        };
        let c = cost_of(&price(), &e).unwrap();
        assert!((c - 0.3).abs() < 1e-9, "{}", c);
    }

    #[test]
    fn cache_writes_cost_more_than_fresh_input() {
        let e = UsageEntry {
            input_tokens: 1_000_000,
            cache_write_tokens: 1_000_000,
            ..UsageEntry::now("chat", "anthropic", "m")
        };
        let c = cost_of(&price(), &e).unwrap();
        assert!((c - 3.75).abs() < 1e-9, "{}", c);
    }

    #[test]
    fn cache_counters_never_exceed_the_input_total() {
        let e = UsageEntry {
            input_tokens: 10,
            cache_read_tokens: 999,
            cache_write_tokens: 999,
            ..UsageEntry::now("chat", "anthropic", "m")
        };
        // 10 tokens no preco de leitura de cache, nada negativo.
        let c = cost_of(&price(), &e).unwrap();
        assert!((c - 10.0 * 0.3 / 1e6).abs() < 1e-12, "{}", c);
    }

    #[test]
    fn model_without_a_price_has_no_cost() {
        let mut p = price();
        p.input_per_m = None;
        assert!(cost_of(&p, &UsageEntry::now("chat", "x", "m")).is_none());
    }
}
