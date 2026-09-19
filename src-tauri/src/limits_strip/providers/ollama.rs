//! Ollama, the runtime on this machine.
//!
//! No credential. One loopback request: `GET http://127.0.0.1:11434/api/ps`,
//! the models held in memory right now. Reply: `models[]` with `name`, `size`,
//! `size_vram`, `context_length`, `expires_at` (ISO) and
//! `details.{parameter_size, quantization_level}`. Nothing leaves the machine,
//! and a runtime that is not running is a quiet reading, not an error.

use crate::limits_strip::{iso_ms, json_or_error, LocalModel, ReadError, Reading, UsageProvider};

const ENDPOINT: &str = "http://127.0.0.1:11434/api/ps";

pub struct Ollama;

pub fn parse_ps(v: &serde_json::Value) -> Vec<LocalModel> {
    v.get("models")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = m.get("name").or_else(|| m.get("model"))?.as_str()?;
            let text = |p: &str| m.pointer(p).and_then(|x| x.as_str()).map(str::to_string);
            Some(LocalModel {
                name: name.to_string(),
                size_bytes: m.get("size").and_then(|x| x.as_u64()),
                vram_bytes: m.get("size_vram").and_then(|x| x.as_u64()),
                context: m.get("context_length").and_then(|x| x.as_u64()),
                quant: text("/details/quantization_level"),
                params: text("/details/parameter_size"),
                expires_at: iso_ms(m.get("expires_at")),
            })
        })
        .collect()
}

#[async_trait::async_trait]
impl UsageProvider for Ollama {
    fn id(&self) -> &'static str {
        "ollama"
    }
    fn label(&self) -> &'static str {
        "Ollama"
    }
    fn local(&self) -> bool {
        true
    }
    fn beta(&self) -> bool {
        false
    }

    async fn detect(&self) -> bool {
        super::installed(".ollama", "ollama")
    }

    async fn read(&self) -> Result<Reading, ReadError> {
        let Ok(resp) = super::loopback().get(ENDPOINT).send().await else {
            return Ok(super::not_running());
        };
        let local_models = parse_ps(&json_or_error(resp).await?);
        Ok(Reading {
            note: local_models
                .is_empty()
                .then(|| "running, no model loaded".to_string()),
            local_models,
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_models_come_with_their_memory_and_expiry() {
        let v = serde_json::json!({"models":[
            {"name":"qwen3:8b","model":"qwen3:8b","size":6654289920u64,"size_vram":6654289920u64,
             "context_length":8192,"expires_at":"2099-01-01T05:00:00.000000+00:00",
             "details":{"parameter_size":"8.2B","quantization_level":"Q4_K_M"}},
            {"size":1}
        ]});
        let ms = parse_ps(&v);
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].name, "qwen3:8b");
        assert_eq!(ms[0].vram_bytes, Some(6_654_289_920));
        assert_eq!(ms[0].quant.as_deref(), Some("Q4_K_M"));
        assert_eq!(ms[0].expires_at, Some(4_070_926_800_000));
        assert!(parse_ps(&serde_json::json!({})).is_empty());
    }
}
