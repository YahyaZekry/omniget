//! `world_bench_report`: the front-end runner hands the bench JSON back to the
//! process so it can be printed and the app can exit. Owned by `f6-bench`.
//!
//! One command, three jobs. The bench needs a Rust side that (a) pushes binary
//! payloads down a `tauri::ipc::Channel` for the I-5 micro-bench, (b) reports
//! process CPU during a sustained burst, and (c) takes the final JSON. Each of
//! those as its own `#[tauri::command]` would mean three new lines in the
//! `generate_handler!` of `lib.rs`, which is a shared file this agent does not
//! own. So the ops are multiplexed on the `op` field of the payload — the
//! bench is a debug hook that only ever runs with `OMNIGET_WORLD_BENCH` set,
//! and one registered name is cheaper than three handoffs.
//!
//! `Option<Channel<_>>` does not compile: `Channel` reaches a command through
//! `CommandArg`, not through `Deserialize`, and `Option<T>` only gets a
//! `CommandArg` impl via `Deserialize`. So the optionality is spelled out in
//! `MaybeChannel` below, and a caller that only wants to file the final report
//! (the `/world` route's own self-test does exactly that) may leave the
//! argument out entirely.
//!
//! Payloads travel as `InvokeResponseBody::Raw`, which is the binary path of
//! the Tauri 2 IPC (the page receives an `ArrayBuffer`). `Channel<Vec<u8>>`
//! would *look* binary but `Vec<u8>` is `Serialize`, so it would be sent as a
//! JSON array of numbers — several times the bytes and a completely different
//! measurement. That is the whole point of I-5, so it matters.

use tauri::ipc::{Channel, CommandArg, CommandItem, InvokeError, InvokeResponseBody};

/// A `Channel` argument that may be absent. Absent and malformed collapse into
/// the same `None`: the only caller that omits it is the final report, which
/// never sends anything, and a malformed id would fail on the first `send`
/// anyway.
pub struct MaybeChannel(pub Option<Channel<InvokeResponseBody>>);

impl<'de, R: tauri::Runtime> CommandArg<'de, R> for MaybeChannel {
    fn from_command(command: CommandItem<'de, R>) -> Result<Self, InvokeError> {
        Ok(Self(Channel::from_command(command).ok()))
    }
}

/// Biggest payload the bench may ask for. The real users are the world diff
/// (~2 KB) and the TV preview (~194 KB); 1 MiB is far above both and keeps a
/// typo from asking for a gigabyte.
const MAX_PAYLOAD_BYTES: usize = 1024 * 1024;
/// Upper bound for a burst, so `count / hz` cannot turn into a hung job.
const MAX_BURST_MESSAGES: u64 = 2_000;

/// Deterministic filler. Not zeros: a page that reads `byte[0]` gets something
/// that changes per message, which keeps a clever engine from eliding the copy.
fn payload(size: usize, seq: u8) -> Vec<u8> {
    let mut buf = vec![0u8; size];
    if let Some(first) = buf.first_mut() {
        *first = seq;
    }
    if size > 1 {
        for (i, b) in buf.iter_mut().enumerate().skip(1) {
            *b = (i % 251) as u8;
        }
    }
    buf
}

/// Reads `size`/`count`/`hz` out of the request with the bench's limits applied.
fn burst_params(report: &serde_json::Value) -> Result<(usize, u64, f64), String> {
    let size = report.get("size").and_then(|v| v.as_u64()).unwrap_or(2048) as usize;
    if size == 0 || size > MAX_PAYLOAD_BYTES {
        return Err(format!("ERR_WORLD_BENCH_SIZE: {size}"));
    }
    let count = report.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
    if count == 0 || count > MAX_BURST_MESSAGES {
        return Err(format!("ERR_WORLD_BENCH_COUNT: {count}"));
    }
    let hz = report.get("hz").and_then(|v| v.as_f64()).unwrap_or(0.0);
    if !(0.0..=1000.0).contains(&hz) {
        return Err(format!("ERR_WORLD_BENCH_HZ: {hz}"));
    }
    Ok((size, count, hz))
}

#[tauri::command]
pub async fn world_bench_report(
    report: serde_json::Value,
    channel: MaybeChannel,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    // The hook is a debug affordance: without the env var this command does
    // nothing at all, so a stray call from the UI can never kill the app.
    if crate::world_bench::scenario().is_none() {
        return Err("ERR_WORLD_BENCH_DISABLED".into());
    }

    match report.get("op").and_then(|v| v.as_str()) {
        // One payload, no pacing. The front-end times the round trip.
        Some("ipc-echo") => {
            let (size, count, _) = burst_params(&report)?;
            let channel = channel.0.ok_or("ERR_WORLD_BENCH_NO_CHANNEL")?;
            for seq in 0..count {
                channel
                    .send(InvokeResponseBody::Raw(payload(size, seq as u8)))
                    .map_err(|e| format!("ERR_WORLD_BENCH_SEND: {e}"))?;
            }
            Ok(serde_json::json!({ "sent": count, "size": size }))
        }
        // `count` payloads paced at `hz`, with process CPU measured across the
        // whole window. This is the 200 KB @ 15 Hz case of I-5.
        Some("ipc-burst") => {
            let (size, count, hz) = burst_params(&report)?;
            let channel = channel.0.ok_or("ERR_WORLD_BENCH_NO_CHANNEL")?;
            let period = if hz > 0.0 {
                std::time::Duration::from_secs_f64(1.0 / hz)
            } else {
                std::time::Duration::ZERO
            };
            let window = crate::world_bench::CpuWindow::start();
            let started = std::time::Instant::now();
            for seq in 0..count {
                channel
                    .send(InvokeResponseBody::Raw(payload(size, seq as u8)))
                    .map_err(|e| format!("ERR_WORLD_BENCH_SEND: {e}"))?;
                if !period.is_zero() {
                    // Absolute schedule, so a slow send does not make the whole
                    // burst drift and under-report the rate.
                    let due = period * (seq as u32 + 1);
                    if let Some(left) = due.checked_sub(started.elapsed()) {
                        tokio::time::sleep(left).await;
                    }
                }
            }
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            Ok(serde_json::json!({
                "sent": count,
                "size": size,
                "elapsed_ms": (elapsed_ms * 10.0).round() / 10.0,
                "cpu_pct": window.finish(),
            }))
        }
        // Diagnostic beacon injected by the hook: log it, never exit on it.
        Some("page-beacon") => {
            crate::world_bench::note_page_href(
                report.get("href").and_then(|v| v.as_str()).unwrap_or(""),
            );
            tracing::info!(
                "[world-bench] page: href={} ready={} canvas={} error={} text={:?}",
                report.get("href").and_then(|v| v.as_str()).unwrap_or("?"),
                report.get("ready").and_then(|v| v.as_str()).unwrap_or("?"),
                report
                    .get("canvas")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                report
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("nenhum"),
                report.get("text").and_then(|v| v.as_str()).unwrap_or("")
            );
            Ok(serde_json::Value::Null)
        }
        // No `op` (or an unknown one): this is the final report. Print and quit.
        _ => {
            crate::world_bench::finish(&app, report);
            Ok(serde_json::Value::Null)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_has_the_asked_size_and_a_sequence_marker() {
        let buf = payload(2048, 7);
        assert_eq!(buf.len(), 2048);
        assert_eq!(buf[0], 7);
        assert_eq!(buf[1], 1);
        assert_ne!(buf[250], buf[251], "the filler must actually vary");
    }

    #[test]
    fn payload_of_one_byte_does_not_panic() {
        assert_eq!(payload(1, 3), vec![3]);
    }

    #[test]
    fn burst_params_defaults_and_limits() {
        let ok = serde_json::json!({ "size": 204800, "count": 150, "hz": 15 });
        assert_eq!(burst_params(&ok).unwrap(), (204_800, 150, 15.0));

        let defaults = serde_json::json!({});
        assert_eq!(burst_params(&defaults).unwrap(), (2048, 1, 0.0));

        for bad in [
            serde_json::json!({ "size": 0 }),
            serde_json::json!({ "size": 4_000_000 }),
            serde_json::json!({ "count": 0 }),
            serde_json::json!({ "count": 999_999 }),
            serde_json::json!({ "hz": -1 }),
        ] {
            assert!(burst_params(&bad).is_err(), "should reject {bad}");
        }
    }
}
