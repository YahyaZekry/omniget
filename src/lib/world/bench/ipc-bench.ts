// I-5 micro-bench: how much does a binary `tauri::ipc::Channel` cost on the
// worst-case webview? The plan needs two answers before the world ships a
// protocol: the simulation diff (~2 KB at 10 Hz) and the TV preview (~194 KB at
// 15 fps). If the 200 KB case is expensive, the fallbacks are already written
// down in the plan (diff at 5 Hz, preview at 5 fps / 240p).
//
// What is measured: the full round trip, `invoke()` out and the first channel
// message back, timed with `performance.now()` on the page side. That includes
// the command dispatch, so it is pessimistic by one invoke — which is what a
// real caller pays anyway.
//
// CPU is *not* measured here. The page cannot see the process it lives in, and
// on Linux the WebGL work happens in a `WebKitWebProcess` the page cannot see
// either. Rust samples `/proc` (and `ps` on macOS) around the burst and returns
// the percentage; see `src-tauri/src/world_bench.rs`.
//
// Owned by `f6-bench`.

import { Channel, invoke } from "@tauri-apps/api/core";

/** The three payload sizes of I-5, in bytes. */
export const PAYLOAD_SIZES = [2 * 1024, 50 * 1024, 200 * 1024] as const;
/** Round trips per size. 100 is enough for a stable p50 and costs < 1 s. */
export const SAMPLES_PER_SIZE = 100;
/** The TV preview case: 15 Hz for 10 s. */
export const BURST_HZ = 15;
export const BURST_SECONDS = 10;

export interface IpcBenchResult {
  /** p50 round trip, in ms, one key per payload size. */
  "2kb_p50_ms": number;
  "50kb_p50_ms": number;
  "200kb_p50_ms": number;
  /** CPU of the app process (plus children) during the 200 KB @ 15 Hz burst. */
  "200kb_15hz_cpu_pct": number;
  /** How many of the burst messages actually arrived. */
  burst_received: number;
  burst_sent: number;
  /** False means the payload came back as a JSON array, not an ArrayBuffer. */
  binary: boolean;
  error?: string;
}

/**
 * Linear-interpolation-free percentile: the lower-bound sample, which is what a
 * p50/p95 of a latency list should be. Sorts a copy; the caller keeps its order.
 */
export function percentile(values: number[], p: number): number {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  const index = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[index];
}

/** Rounds to `digits` decimals so the JSON stays readable and diffable. */
export function round(value: number, digits = 3): number {
  const factor = 10 ** digits;
  return Math.round(value * factor) / factor;
}

function byteLength(message: unknown): number {
  if (message instanceof ArrayBuffer) return message.byteLength;
  if (ArrayBuffer.isView(message)) return message.byteLength;
  if (Array.isArray(message)) return message.length;
  return 0;
}

function isBinary(message: unknown): boolean {
  return message instanceof ArrayBuffer || ArrayBuffer.isView(message);
}

/**
 * A round trip that never comes back is a hung job, and a hung job reports
 * nothing. Past this the sample is abandoned and the bench moves on.
 */
export const ECHO_TIMEOUT_MS = 5_000;

/** One `invoke` + one channel message, timed end to end. */
function echoOnce(size: number): Promise<{ ms: number; bytes: number; binary: boolean }> {
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      reject(new Error(`ERR_IPC_ECHO_TIMEOUT:${size}`));
    }, ECHO_TIMEOUT_MS);
    const channel = new Channel<unknown>((message) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve({ ms: performance.now() - started, bytes: byteLength(message), binary: isBinary(message) });
    });
    const started = performance.now();
    invoke("world_bench_report", { report: { op: "ipc-echo", size, count: 1 }, channel }).catch((e) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(e instanceof Error ? e : new Error(String(e)));
    });
  });
}

/**
 * Runs the whole I-5 bench. Never throws: a failure lands in `error` and the
 * numbers stay 0, because the bench reports, it does not decide.
 */
export async function runIpcBench(): Promise<IpcBenchResult> {
  const result: IpcBenchResult = {
    "2kb_p50_ms": 0,
    "50kb_p50_ms": 0,
    "200kb_p50_ms": 0,
    "200kb_15hz_cpu_pct": 0,
    burst_received: 0,
    burst_sent: 0,
    binary: false,
  };

  try {
    let sawBinary = true;
    const keys = ["2kb_p50_ms", "50kb_p50_ms", "200kb_p50_ms"] as const;
    for (let i = 0; i < PAYLOAD_SIZES.length; i++) {
      const size = PAYLOAD_SIZES[i];
      const samples: number[] = [];
      for (let s = 0; s < SAMPLES_PER_SIZE; s++) {
        const one = await echoOnce(size);
        // The first round trip pays for the command being resolved for the
        // first time; it is a warm-up, not a sample.
        if (s > 0) samples.push(one.ms);
        if (!one.binary) sawBinary = false;
      }
      result[keys[i]] = round(percentile(samples, 50));
    }
    result.binary = sawBinary;

    const count = BURST_HZ * BURST_SECONDS;
    let received = 0;
    const channel = new Channel<unknown>(() => {
      received += 1;
    });
    const burst = (await invoke("world_bench_report", {
      report: { op: "ipc-burst", size: PAYLOAD_SIZES[2], count, hz: BURST_HZ },
      channel,
    })) as { cpu_pct?: number | null; sent?: number } | null;
    // Messages are delivered through the IPC queue, so the last few can still
    // be in flight when the command resolves.
    await new Promise((r) => setTimeout(r, 250));
    result.burst_sent = burst?.sent ?? count;
    result.burst_received = received;
    result["200kb_15hz_cpu_pct"] = round(burst?.cpu_pct ?? 0, 1);
  } catch (e) {
    result.error = e instanceof Error ? e.message : String(e);
  }

  return result;
}
