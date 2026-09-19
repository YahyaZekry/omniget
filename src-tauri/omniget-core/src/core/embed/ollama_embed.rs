//! Alternative embedding source: `nomic-embed-text` on a local Ollama.
//!
//! Only used when the user picks it in Settings (`EmbedSource::Ollama`); the
//! default stays the ONNX model we manage. Nothing here starts, installs or
//! pings Ollama on its own — the host comes from `tools/ollama.rs`, and a
//! request only happens when somebody asks for vectors.
//!
//! Two endpoints, because both exist in the wild: `/api/embed` (batched, newer)
//! and `/api/embeddings` (one prompt per call, older). We try the batched one
//! and fall back on 404/405.

use serde_json::json;

use super::{EmbedError, ERR_EMBED_OLLAMA};

/// `nomic-embed-text` is 768-d, which is why a stored vector must carry the
/// source that produced it.
pub const DIM: usize = 768;

pub const DEFAULT_MODEL: &str = "nomic-embed-text";

/// Same 64 as the ONNX path: one HTTP request per chunk.
pub const BATCH: usize = 64;

fn base(host: Option<&str>) -> String {
    let raw = host
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::core::tools::ollama::DEFAULT_HOST);
    raw.trim_end_matches('/').to_string()
}

/// Reads `{"embeddings": [[..], ..]}` (`/api/embed`).
pub fn parse_batch(body: &serde_json::Value, expected: usize) -> Result<Vec<Vec<f32>>, EmbedError> {
    let arr = body
        .get("embeddings")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            EmbedError::new(
                ERR_EMBED_OLLAMA,
                format!("answer without `embeddings`: {}", short(body)),
            )
        })?;
    if arr.len() != expected {
        return Err(EmbedError::new(
            ERR_EMBED_OLLAMA,
            format!("asked for {expected} vectors, got {}", arr.len()),
        ));
    }
    arr.iter().map(numbers).collect()
}

/// Reads `{"embedding": [..]}` (`/api/embeddings`).
pub fn parse_single(body: &serde_json::Value) -> Result<Vec<f32>, EmbedError> {
    let v = body.get("embedding").ok_or_else(|| {
        EmbedError::new(
            ERR_EMBED_OLLAMA,
            format!("answer without `embedding`: {}", short(body)),
        )
    })?;
    numbers(v)
}

fn numbers(v: &serde_json::Value) -> Result<Vec<f32>, EmbedError> {
    let arr = v
        .as_array()
        .ok_or_else(|| EmbedError::new(ERR_EMBED_OLLAMA, "a vector came back as a non-array"))?;
    let mut out = Vec::with_capacity(arr.len());
    for n in arr {
        out.push(n.as_f64().ok_or_else(|| {
            EmbedError::new(ERR_EMBED_OLLAMA, "a vector has a non-numeric element")
        })? as f32);
    }
    if out.is_empty() {
        return Err(EmbedError::new(
            ERR_EMBED_OLLAMA,
            "an empty vector came back",
        ));
    }
    Ok(out)
}

fn short(v: &serde_json::Value) -> String {
    let s = v.to_string();
    if s.len() > 200 {
        format!("{}…", &s[..200])
    } else {
        s
    }
}

/// L2-normalizes in place, so an Ollama vector is comparable with `cosine` the
/// same way the ONNX ones are. A zero vector stays zero.
pub fn normalize(v: &mut [f32]) {
    let norm = v
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt();
    if norm <= 0.0 {
        return;
    }
    for x in v.iter_mut() {
        *x = (*x as f64 / norm) as f32;
    }
}

/// Embeddings from a local Ollama. `host`/`model` default to
/// `http://127.0.0.1:11434` and `nomic-embed-text`.
pub async fn embed_batch(
    host: Option<&str>,
    model: Option<&str>,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>, EmbedError> {
    if texts.is_empty() {
        return Ok(vec![]);
    }
    let b = base(host);
    let model = model
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_MODEL);
    let client = crate::core::tools::client()
        .map_err(|e| EmbedError::new(ERR_EMBED_OLLAMA, e.to_string()))?;

    let mut out: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
    let mut batched_supported = true;
    for chunk in texts.chunks(BATCH) {
        if batched_supported {
            let resp = client
                .post(format!("{b}/api/embed"))
                .json(&json!({ "model": model, "input": chunk }))
                .send()
                .await
                .map_err(|e| {
                    EmbedError::new(ERR_EMBED_OLLAMA, format!("{b} did not answer: {e}"))
                })?;
            let status = resp.status();
            if status.is_success() {
                let body: serde_json::Value = resp.json().await.map_err(|e| {
                    EmbedError::new(ERR_EMBED_OLLAMA, format!("unreadable answer: {e}"))
                })?;
                let mut vs = parse_batch(&body, chunk.len())?;
                for v in &mut vs {
                    normalize(v);
                }
                out.extend(vs);
                continue;
            }
            if status.as_u16() != 404 && status.as_u16() != 405 {
                return Err(http_error(&b, status, resp.text().await.ok()));
            }
            // Old Ollama: no `/api/embed`. One call per text from here on.
            batched_supported = false;
        }
        for text in chunk {
            let resp = client
                .post(format!("{b}/api/embeddings"))
                .json(&json!({ "model": model, "prompt": text }))
                .send()
                .await
                .map_err(|e| {
                    EmbedError::new(ERR_EMBED_OLLAMA, format!("{b} did not answer: {e}"))
                })?;
            let status = resp.status();
            if !status.is_success() {
                return Err(http_error(&b, status, resp.text().await.ok()));
            }
            let body: serde_json::Value = resp.json().await.map_err(|e| {
                EmbedError::new(ERR_EMBED_OLLAMA, format!("unreadable answer: {e}"))
            })?;
            let mut v = parse_single(&body)?;
            normalize(&mut v);
            out.push(v);
        }
    }
    Ok(out)
}

fn http_error(host: &str, status: reqwest::StatusCode, body: Option<String>) -> EmbedError {
    let hint = if status.as_u16() == 404 {
        format!(" — is the `{DEFAULT_MODEL}` model pulled?")
    } else {
        String::new()
    };
    let tail = body
        .map(|b| b.chars().take(200).collect::<String>())
        .unwrap_or_default();
    EmbedError::new(
        ERR_EMBED_OLLAMA,
        format!("{host} answered HTTP {status}{hint} {tail}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::embed::cosine;

    #[test]
    fn o_host_padrao_e_o_do_modulo_do_ollama_e_a_barra_final_some() {
        assert_eq!(base(None), crate::core::tools::ollama::DEFAULT_HOST);
        assert_eq!(base(Some("  ")), crate::core::tools::ollama::DEFAULT_HOST);
        assert_eq!(base(Some("http://x:1/")), "http://x:1");
    }

    #[test]
    fn le_a_resposta_em_lote() {
        let body = serde_json::json!({"embeddings": [[1.0, 0.0], [0.0, 2.0]]});
        let v = parse_batch(&body, 2).expect("lote");
        assert_eq!(v, vec![vec![1.0, 0.0], vec![0.0, 2.0]]);
    }

    #[test]
    fn lote_com_contagem_errada_e_erro() {
        let body = serde_json::json!({"embeddings": [[1.0]]});
        let e = parse_batch(&body, 2).expect_err("contagem");
        assert_eq!(e.code, ERR_EMBED_OLLAMA);
        assert!(e.message.contains("got 1"));
    }

    #[test]
    fn resposta_sem_campo_esperado_e_erro_com_trecho_do_corpo() {
        let body = serde_json::json!({"error": "model not found"});
        let e = parse_batch(&body, 1).expect_err("sem campo");
        assert!(e.message.contains("model not found"));
        assert_eq!(parse_single(&body).unwrap_err().code, ERR_EMBED_OLLAMA);
    }

    #[test]
    fn le_a_resposta_antiga_de_um_vetor_so() {
        let body = serde_json::json!({"embedding": [0.5, 0.5]});
        assert_eq!(parse_single(&body).expect("single"), vec![0.5, 0.5]);
    }

    #[test]
    fn vetor_vazio_ou_com_lixo_e_erro() {
        assert!(parse_single(&serde_json::json!({"embedding": []})).is_err());
        assert!(parse_single(&serde_json::json!({"embedding": ["a"]})).is_err());
        assert!(parse_single(&serde_json::json!({"embedding": 3})).is_err());
    }

    #[test]
    fn normalizar_deixa_o_cosseno_comparavel_com_o_do_onnx() {
        let mut v = vec![3.0f32, 4.0];
        normalize(&mut v);
        assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-6);
        let mut w = vec![30.0f32, 40.0];
        normalize(&mut w);
        assert!((cosine(&v, &w) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn normalizar_vetor_nulo_nao_gera_nan() {
        let mut v = vec![0.0f32; 4];
        normalize(&mut v);
        assert!(v.iter().all(|x| *x == 0.0));
    }

    #[test]
    fn erro_http_404_sugere_puxar_o_modelo() {
        let e = http_error("http://h", reqwest::StatusCode::NOT_FOUND, None);
        assert!(e.message.contains(DEFAULT_MODEL));
    }

    #[test]
    fn a_dimensao_do_nomic_nao_e_a_do_minilm() {
        assert_eq!(DIM, 768);
        assert_ne!(DIM, super::super::DIM);
    }

    /// Precisa de um Ollama local com o modelo puxado. O modelo vem de
    /// `OMNIGET_EMBED_OLLAMA_MODEL` (padrão `nomic-embed-text`) e o host de
    /// `OMNIGET_EMBED_OLLAMA_HOST`, para dar para exercitar o caminho HTTP com
    /// o modelo que a máquina já tem:
    ///   OMNIGET_EMBED_OLLAMA_MODEL=qwen3:0.6b \
    ///   cargo test -p omniget-core --features desktop embed:: -- --ignored
    #[test]
    #[ignore = "rede local: precisa de Ollama rodando com o modelo de embedding puxado"]
    fn ollama_local_devolve_vetores_normalizados() {
        let modelo = std::env::var("OMNIGET_EMBED_OLLAMA_MODEL").ok();
        let host = std::env::var("OMNIGET_EMBED_OLLAMA_HOST").ok();
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let v = rt
            .block_on(embed_batch(
                host.as_deref(),
                modelo.as_deref(),
                &["olá mundo", "hello world"],
            ))
            .expect("embed");
        assert_eq!(v.len(), 2);
        assert!(v[0].len() >= 256, "vetor curto demais: {}", v[0].len());
        assert_eq!(v[0].len(), v[1].len());
        let n: f32 = v[0].iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-3, "não veio normalizado: {n}");
        // Textos diferentes não podem sair idênticos.
        assert!(cosine(&v[0], &v[1]) < 0.9999);
    }
}
