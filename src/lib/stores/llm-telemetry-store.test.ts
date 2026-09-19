import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const unlisten = vi.fn();
const listen = vi.fn(async (_event: string, _cb: (ev: unknown) => void) => unlisten);

vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, cb: (ev: unknown) => void) => listen(event, cb),
}));

type Store = typeof import("./llm-telemetry-store.svelte");

let store: Store;

beforeAll(async () => {
  vi.stubGlobal("$state", <T>(value: T) => value);
  vi.stubGlobal("$derived", <T>(value: T) => value);
  store = await import("./llm-telemetry-store.svelte");
});

afterEach(() => {
  invoke.mockReset();
  listen.mockClear();
  unlisten.mockClear();
  store.resetTelemetryStore();
});

afterAll(() => {
  vi.unstubAllGlobals();
});

const NOW = 1_800_000_000_000;

function pt(tMs: number, tokens = 10, tps = 20, cost = 0.001) {
  return { t_ms: tMs, tokens_out: tokens, tokens_in: tokens * 2, tps, cost_usd: cost };
}

describe("aggregateWindow", () => {
  it("sums counters into the right bucket and averages the rate", () => {
    const span = 60_000;
    const points = [
      pt(NOW - 59_000, 10, 30),
      pt(NOW - 58_000, 5, 10),
      pt(NOW - 1_000, 7, 50),
    ];
    const buckets = store.aggregateWindow(points, NOW, span, 4);
    expect(buckets).toHaveLength(4);
    // The two oldest samples land in the first 15 s slice.
    expect(buckets[0].tokens_out).toBe(15);
    expect(buckets[0].tokens_in).toBe(30);
    expect(buckets[0].samples).toBe(2);
    expect(buckets[0].tps).toBe(20); // mean of 30 and 10, not the sum
    // The newest lands in the last one.
    expect(buckets[3].tokens_out).toBe(7);
    expect(buckets[3].tps).toBe(50);
    // A quiet slice reads as zero instead of carrying the previous value.
    expect(buckets[1].samples).toBe(0);
    expect(buckets[1].tps).toBe(0);
  });

  it("drops samples outside the window and keeps bucket timestamps ordered", () => {
    const buckets = store.aggregateWindow([pt(NOW - 10 * 3_600_000), pt(NOW + 5_000)], NOW, 3_600_000, 6);
    expect(buckets.every((b) => b.samples === 0)).toBe(true);
    for (let i = 1; i < buckets.length; i++) expect(buckets[i].t_ms).toBeGreaterThan(buckets[i - 1].t_ms);
  });

  it("never returns more than MAX_POINTS points", () => {
    expect(store.aggregateWindow([], NOW, store.WINDOWS["24h"], 5000)).toHaveLength(store.MAX_POINTS);
    expect(store.aggregateWindow([], NOW, store.WINDOWS["24h"], 0)).toHaveLength(1);
  });

  it("windowTotals adds the counters and takes the peak rate", () => {
    const buckets = store.aggregateWindow([pt(NOW - 30_000, 10, 12), pt(NOW - 5_000, 4, 44)], NOW, 60_000, 4);
    const totals = store.windowTotals(buckets);
    expect(totals.tokens_out).toBe(14);
    expect(totals.tokens_in).toBe(28);
    expect(totals.tps_peak).toBe(44);
    expect(totals.cost_usd).toBeCloseTo(0.002, 6);
  });

  it("aggregates the 24 h fake history into 96 buckets with the full token count", () => {
    const snap = store.fakeSnapshot(NOW);
    const agent = snap.agents[0];
    expect(agent.history).toHaveLength(288);
    const buckets = store.aggregateWindow(agent.history, snap.ts_ms, store.WINDOWS["24h"], 96);
    expect(buckets).toHaveLength(96);
    const totals = store.windowTotals(buckets);
    const raw = agent.history.reduce((s, p) => s + p.tokens_out, 0);
    // The newest sample sits exactly on `now` and is kept.
    expect(totals.tokens_out).toBe(raw);
  });
});

describe("context guardian bands", () => {
  it("splits at 70% and 90%", () => {
    expect(store.guardianBand(0)).toBe("ok");
    expect(store.guardianBand(0.6999)).toBe("ok");
    expect(store.guardianBand(0.7)).toBe("warn");
    expect(store.guardianBand(0.9)).toBe("warn");
    expect(store.guardianBand(0.9001)).toBe("critical");
    expect(store.guardianBand(1)).toBe("critical");
  });

  it("clamps the ratio and treats an unknown window as 0", () => {
    expect(store.contextRatio(64_000, 128_000)).toBe(0.5);
    expect(store.contextRatio(300_000, 128_000)).toBe(1);
    expect(store.contextRatio(-5, 128_000)).toBe(0);
    expect(store.contextRatio(1_000, 0)).toBe(0);
    expect(store.guardianBand(store.contextRatio(1_000, 0))).toBe("ok");
  });

  it("puts the three fake agents in the three bands", () => {
    const [a, b, c] = store.fakeSnapshot(NOW).agents;
    expect(store.guardianBand(store.contextRatio(a.context_used_tokens, a.context_window))).toBe("ok");
    expect(store.guardianBand(store.contextRatio(b.context_used_tokens, b.context_window))).toBe("warn");
    expect(store.guardianBand(store.contextRatio(c.context_used_tokens, c.context_window))).toBe("critical");
  });
});

describe("cacheHitRate", () => {
  it("is cached / (cached + fresh) and 0 when nothing was counted", () => {
    expect(store.cacheHitRate({ cache_read_tokens: 30, input_tokens: 70 })).toBeCloseTo(0.3, 6);
    expect(store.cacheHitRate({ cache_read_tokens: 0, input_tokens: 0 })).toBe(0);
  });
});

describe("pushHistory", () => {
  it("merges by timestamp, sorts and bounds the ring", () => {
    const merged = store.pushHistory([pt(3), pt(1)], [pt(2), pt(1, 99)]);
    expect(merged.map((p) => p.t_ms)).toEqual([1, 2, 3]);
    expect(merged[0].tokens_out).toBe(99); // the newest sample for a timestamp wins
    const long = store.pushHistory([], Array.from({ length: 50 }, (_, i) => pt(i)), 10);
    expect(long).toHaveLength(10);
    expect(long[0].t_ms).toBe(40);
  });
});

describe("fakeSnapshot", () => {
  it("is deterministic for the same sample grid", () => {
    const a = store.fakeSnapshot(NOW);
    const b = store.fakeSnapshot(NOW + 1_000);
    expect(b.ts_ms).toBe(a.ts_ms);
    expect(JSON.stringify(b)).toBe(JSON.stringify(a));
  });

  it("carries a fallback chain with exactly one active candidate", () => {
    const snap = store.fakeSnapshot(NOW);
    expect(snap.fallback_chain).toHaveLength(snap.quotas.length);
    expect(snap.quotas.filter((q) => q.active)).toHaveLength(1);
    expect(snap.quotas.some((q) => q.used > 0.9)).toBe(true);
  });

  it("has a reroute event so the timeline shows the switch", () => {
    const snap = store.fakeSnapshot(NOW);
    expect(snap.agents[1].events.some((e) => e.kind === "rerouted" && e.code === "ERR_LLM_RATE")).toBe(true);
  });
});

describe("store lifecycle", () => {
  it("reports the backend stub as unavailable and keeps the page empty", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    await store.loadTelemetry();
    expect(store.getTelemetryState()).toBe("unavailable");
    expect(store.getTelemetryError()).toBe("ERR_STUB");
    expect(store.getTelemetry().agents).toHaveLength(0);
  });

  it("keeps the history across two snapshots of the same agent", async () => {
    const base = {
      agent_id: "a1",
      agent_name: "A",
      provider: "p",
      model: "m",
      state: "idle",
      turns: 1,
      errors: 0,
      input_tokens: 0,
      output_tokens: 0,
      cache_read_tokens: 0,
      cache_write_tokens: 0,
      first_token_ms_p50: null,
      first_token_ms_last: null,
      tps_last: null,
      cost_usd: 0,
      context_used_tokens: 0,
      context_window: 1000,
      context_estimated: false,
      last_error_code: null,
      events: [],
    };
    invoke.mockResolvedValueOnce({ ts_ms: NOW, agents: [{ ...base, history: [pt(1)] }], quotas: [], cost_by_day: [], fallback_chain: [] });
    await store.loadTelemetry();
    invoke.mockResolvedValueOnce({ ts_ms: NOW + 500, agents: [{ ...base, history: [pt(2)] }], quotas: [], cost_by_day: [], fallback_chain: [] });
    await store.loadTelemetry();
    expect(store.getTelemetry().agents[0].history.map((p) => p.t_ms)).toEqual([1, 2]);
    expect(store.getTelemetryState()).toBe("ready");
  });

  it("subscribes to llm://telemetry once and unsubscribes on stop — no timer of its own", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    const stop = await store.startTelemetry();
    expect(listen).toHaveBeenCalledTimes(1);
    expect(listen.mock.calls[0][0]).toBe("llm://telemetry");
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("demo mode fills the page without calling the backend", async () => {
    await store.startTelemetry({ demo: true });
    expect(invoke).not.toHaveBeenCalled();
    expect(listen).not.toHaveBeenCalled();
    expect(store.getTelemetry().agents).toHaveLength(3);
    expect(store.isDemo()).toBe(true);
  });

  it("falls back to the usage ledger when the snapshot brings no cost", async () => {
    invoke.mockResolvedValueOnce({ ts_ms: NOW, agents: [], quotas: [], cost_by_day: [], fallback_chain: [] });
    await store.loadTelemetry();
    invoke.mockResolvedValueOnce({ by_day: [{ key: "2026-09-17", cost_usd: 1.5, calls: 9 }] });
    await store.loadCostFallback();
    expect(invoke.mock.calls[1][0]).toBe("tool_usage_report");
    expect(store.getTelemetry().cost_by_day).toEqual([{ day: "2026-09-17", cost_usd: 1.5, calls: 9 }]);
  });

  it("resetting an agent clears its history and timeline only", async () => {
    await store.startTelemetry({ demo: true });
    store.resetAgentHistory("researcher");
    const agents = store.getTelemetry().agents;
    expect(agents.find((a) => a.agent_id === "researcher")?.history).toHaveLength(0);
    expect(agents.find((a) => a.agent_id === "coordinator")?.history).toHaveLength(288);
  });
});
