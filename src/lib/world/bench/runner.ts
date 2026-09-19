// World bench runner: calibrates, runs the synthetic scene, measures first frame and
// IPC, then hands the JSON to `world_bench_report`. Owned by `f6-bench`
// (format: docs/agents/f6-bench.md §4).
//
// This file is the front-end half of `OMNIGET_WORLD_BENCH`. The Rust half
// (`src-tauri/src/world_bench.rs`) navigates the main webview to
// `/world?bench=<scenario>`, the route calls `runWorldBench(canvas, scenario)`,
// and the last thing that happens here is an `invoke('world_bench_report')`
// that makes the process print one `[world-bench] {json}` line and exit.
//
// Rule of the house: **the bench reports, it does not decide.** A renderer that
// throws, a WebGL2 context that never creates, an IPC channel that dies — all of
// those become fields in the JSON (`error`, `backend: "none"`, zeros), never an
// exception that leaves the CI job with no data at all. The only thing allowed
// to fail the job is `scripts/world-bench.mjs` comparing against the baseline.
//
// `$lib/world/render` is imported dynamically, so nothing of the renderer is in
// the bundle of an app that never opens `/world`.

import { invoke, Channel } from "@tauri-apps/api/core";
import { runIpcBench, percentile, round, type IpcBenchResult } from "./ipc-bench";
import type { FrameStats, Scene, Tier } from "$lib/world/render/types";

/** 20 s of scene, as the plan's §1.4 asks for. */
export const SCENE_SECONDS = 20;
/** Agents on screen for the tier-0 budget line ("8 agentes ≥ 20 fps"). */
export const SCENE_SPRITES = 8;
/** 3×3 chunks around the camera. */
export const SCENE_CHUNKS = 9;
/** Text lines in the synthetic scene (the calibration scene uses the same 32). */
export const SCENE_TEXTS = 32;

export interface SceneReport {
  sprites: number;
  chunks: number;
  median_frame_ms: number;
  p95_frame_ms: number;
  /** Janela medida dividida pelos frames — o fps vem daqui, nao da mediana. */
  avg_frame_ms: number;
  cpu_ms: number;
  fps: number;
  draw_calls: number;
  frames: number;
}

/**
 * I-12 in one object: does a WebGL2 context create at all on this webview?
 * Deliberately independent of `$lib/world/render` — the incognita is about the
 * runner, not about the renderer, and the two must be able to fail separately.
 */
export interface WebglProbe {
  available: boolean;
  /** `true` when the context only came back without `failIfMajorPerformanceCaveat`. */
  caveat_only: boolean;
  /** WebGL1 as a consolation prize, when 2 is not there. */
  webgl1: boolean;
  /**
   * Masked on WebKitGTK since 2.46 (always "Apple GPU"/"Apple Inc.", estudo 73
   * §B.2), so this is logged and never used to decide anything.
   */
  renderer: string;
  vendor: string;
  version: string;
  max_texture_size: number;
  error?: string;
}

export interface WorldBenchReport {
  scenario: string;
  backend: string;
  tier: Tier;
  calibrate_median_ms: number;
  context_lost_during_calibration: boolean;
  scene: SceneReport;
  first_frame_ms: number;
  /** Intervals over `STALL_MS`: the rAF clock was throttled or suspended. */
  raf_stalls: number;
  /** True when the loop *ended* driving itself instead of on rAF. */
  raf_pumped: boolean;
  /** Times the loop got vsync back after having to pump itself. */
  raf_recoveries: number;
  /** Ms descontados da janela por espera de rAF que nao veio. */
  raf_wasted_ms: number;
  webgl2: WebglProbe;
  ipc: IpcBenchResult;
  error?: string;
}

/**
 * Creates and immediately drops a WebGL2 context on a throwaway canvas, so the
 * canvas the renderer gets is untouched (a canvas remembers its first context
 * forever). Answers I-12 whatever the renderer does.
 */
export function probeWebgl2(): WebglProbe {
  const probe: WebglProbe = {
    available: false,
    caveat_only: false,
    webgl1: false,
    renderer: "",
    vendor: "",
    version: "",
    max_texture_size: 0,
  };
  try {
    const canvas = document.createElement("canvas");
    canvas.width = 64;
    canvas.height = 64;
    // Asking with the caveat flag first is the only place a browser is honest
    // about software rendering. Estudo 73 §B.1 says WebKitGTK creates the
    // context anyway, so a difference between the two calls is itself a finding.
    let gl = canvas.getContext("webgl2", { failIfMajorPerformanceCaveat: true }) as WebGL2RenderingContext | null;
    if (!gl) {
      gl = canvas.getContext("webgl2") as WebGL2RenderingContext | null;
      probe.caveat_only = gl !== null;
    }
    if (!gl) {
      probe.webgl1 = canvas.getContext("webgl") !== null;
      return probe;
    }
    probe.available = true;
    probe.webgl1 = true;
    probe.renderer = String(gl.getParameter(gl.RENDERER) ?? "");
    probe.vendor = String(gl.getParameter(gl.VENDOR) ?? "");
    probe.version = String(gl.getParameter(gl.VERSION) ?? "");
    probe.max_texture_size = Number(gl.getParameter(gl.MAX_TEXTURE_SIZE) ?? 0);
    (gl.getExtension("WEBGL_lose_context") as { loseContext(): void } | null)?.loseContext();
  } catch (e) {
    probe.error = e instanceof Error ? e.message : String(e);
  }
  return probe;
}

const EMPTY_SCENE: SceneReport = {
  sprites: 0,
  chunks: 0,
  median_frame_ms: 0,
  p95_frame_ms: 0,
  avg_frame_ms: 0,
  cpu_ms: 0,
  fps: 0,
  draw_calls: 0,
  frames: 0,
};

/** A frame this slow is not a slow frame, it is a stopped clock. */
export const STALL_MS = 250;
/** Consecutive stalls after which the loop stops waiting on rAF and pumps itself. */
export const STALL_LIMIT = 3;
/**
 * Pumped frames between two attempts to go back to vsync. The throttle that
 * forces the pump is usually transient — the window is still coming to the
 * front when the loop starts — and latching the pump for the whole run would
 * turn a 60 fps machine into a throughput number forever.
 */
export const RAF_RETRY_EVERY = 120;

/**
 * The frame clock, with a pump of its own.
 *
 * WKWebView (and WebKitGTK under a compositor) suspends `requestAnimationFrame`
 * for a window that is occluded or unfocused. The first version of this loop
 * gave up after three such stalls and reported `frames: 0, fps: 0` — while the
 * script printed OK and exited 0. A bench that measures nothing must never look
 * like a bench that measured something, so this version does the opposite of
 * giving up: after `STALL_LIMIT` stalls it stops waiting on rAF and drives the
 * loop with `setTimeout(0)`, recording `raf_stalls` so the report says the
 * frame clock was not vsync.
 *
 * In that mode the interval between frames is work-bound rather than
 * vsync-bound, which is exactly what a throughput measurement wants; what it
 * stops being is a measurement of *presentation*. `raf_stalls > 0` in the JSON
 * is the flag for that, and `docs/bench/world-f6.md` says so.
 */
function makeFrameClock() {
  let stalls = 0;
  let consecutive = 0;
  let pumped = false;
  let sincePump = 0;
  let recoveries = 0;
  /**
   * Tempo gasto esperando um rAF que nao veio (falhas e sondas de volta ao
   * vsync). Nao e tempo de renderizar nada, entao sai da janela que divide o
   * fps — senao o proprio custo de tentar recuperar o vsync vira "frame lento".
   */
  let wastedMs = 0;

  // `setTimeout` is the wrong pump for a webview that is throttling: the same
  // background policy that suspends rAF clamps timers, and a bench pumped by a
  // clamped timer measures the clamp. A `MessageChannel` port is a macrotask
  // the throttler does not touch, so the loop keeps the renderer's own pace.
  const channel = typeof MessageChannel === "function" ? new MessageChannel() : null;

  const macrotask = (): Promise<void> =>
    new Promise((resolve) => {
      if (!channel) {
        setTimeout(resolve, 0);
        return;
      }
      channel.port1.onmessage = () => resolve();
      channel.port2.postMessage(0);
    });

  const next = (): Promise<number> =>
    new Promise((resolve) => {
      if (pumped && ++sincePump < RAF_RETRY_EVERY) {
        void macrotask().then(() => resolve(performance.now()));
        return;
      }
      if (pumped) {
        // Probe: one real rAF wait. If it answers in time, vsync is back.
        sincePump = 0;
        let settled = false;
        const probe = (ok: boolean) => {
          if (settled) return;
          settled = true;
          if (ok) {
            pumped = false;
            consecutive = 0;
            recoveries++;
          } else {
            wastedMs += STALL_MS;
          }
          resolve(performance.now());
        };
        requestAnimationFrame(() => probe(true));
        setTimeout(() => probe(false), STALL_MS);
        return;
      }
      let done = false;
      const finish = (viaTimeout: boolean) => {
        if (done) return;
        done = true;
        if (viaTimeout) {
          stalls++;
          wastedMs += STALL_MS;
          if (++consecutive >= STALL_LIMIT) pumped = true;
        } else {
          consecutive = 0;
        }
        resolve(performance.now());
      };
      requestAnimationFrame(() => finish(false));
      setTimeout(() => finish(true), STALL_MS);
    });

  return {
    next,
    get stalls() {
      return stalls;
    },
    get pumped() {
      return pumped;
    },
    get recoveries() {
      return recoveries;
    },
    get wastedMs() {
      return wastedMs;
    },
  };
}

/**
 * Walks the synthetic agents so the depth sort has real work every frame. A
 * scene whose sprites never move would measure a best case the world never has.
 */
export function stepScene(scene: Scene, elapsedMs: number): void {
  const t = elapsedMs / 1000;
  for (let i = 0; i < scene.sprites.length; i++) {
    const phase = t + (i * Math.PI * 2) / Math.max(1, scene.sprites.length);
    scene.sprites[i].x += Math.cos(phase) * 0.02;
    scene.sprites[i].y += Math.sin(phase) * 0.02;
  }
}

/**
 * The scene half of the bench, isolated so a renderer failure cannot take the
 * IPC numbers down with it. Drives the renderer by hand instead of through
 * `createLoop` because the bench needs every raw frame delta, not a smoothed
 * fps, and must not let the watchdog change tier mid-measurement.
 */
async function runScene(
  canvas: HTMLCanvasElement,
  render: typeof import("$lib/world/render"),
): Promise<{
  backend: string;
  tier: Tier;
  calibrateMedianMs: number;
  contextLost: boolean;
  scene: SceneReport;
  firstFrameMs: number;
  stalls: number;
  pumped: boolean;
  recoveries: number;
  wastedMs: number;
}> {
  // `null`: calibration makes its own offscreen canvas. Handing it the bench
  // canvas would burn the canvas's one and only context type on the probe.
  const calibration = await render.calibrate(null);

  const renderer = render.createRenderer();
  const caps = await renderer.init(canvas, { tier: calibration.tier });

  const atlas = await render.createSyntheticAtlas();
  renderer.loadAtlas(atlas.json, atlas.pages);

  const scene = render.buildBenchScene({
    sprites: SCENE_SPRITES,
    chunks: SCENE_CHUNKS,
    texts: SCENE_TEXTS,
  });
  render.primeBenchChunks(renderer, scene);
  render.attachBenchTexts(renderer, scene, SCENE_TEXTS);
  renderer.resize(canvas.width, canvas.height, 1);
  scene.camera.width = canvas.width;
  scene.camera.height = canvas.height;

  const frameMs: number[] = [];
  const cpuMs: number[] = [];
  let drawCalls = 0;
  let firstFrameMs = 0;
  /** Every call to `renderer.frame()`, cold frame included. */
  let frames = 0;

  const clock = makeFrameClock();
  let previous = await clock.next();
  const started = previous;
  while (previous - started < SCENE_SECONDS * 1000) {
    const now = await clock.next();
    const dt = now - previous;
    previous = now;
    // Every tick draws, stalled or not: `frames` has to be the number of frames
    // this bench actually rendered, and a loop that skips work to protect its
    // own percentiles is a loop that can report zero and still exit green.
    stepScene(scene, now - started);
    const stats: FrameStats = renderer.frame(scene, dt);
    frames++;
    if (firstFrameMs === 0) {
      // `performance.now()` counts from the navigation that opened `/world`, so
      // this already contains module load, atlas build and calibration: exactly
      // the "primeiro frame ao abrir a rota" of §1.2.
      firstFrameMs = round(performance.now(), 1);
    } else {
      // The first frame is the cold one; it has its own line in the report and
      // does not belong in the steady-state percentiles. A stalled interval is
      // left out of the *interval* percentiles for the same reason — it
      // measures the throttle, not the renderer — but its CPU cost is real and
      // counts, and the frame itself is counted in `frames`.
      if (dt < STALL_MS) frameMs.push(dt);
      cpuMs.push(stats.cpuMs);
    }
    drawCalls = stats.drawCalls;
  }

  renderer.destroy();

  // fps vem do tempo decorrido dividido pelos frames, nao de 1000/mediana: o
  // `performance.now()` do WKWebView tem resolucao de 1 ms, entao um frame de
  // 0,3 ms aparece como 0 e a mediana de uma cena rapida vira 0 — fps
  // infinito, ou zero, dependendo de onde a divisao cai. Media de janela nao
  // tem esse buraco.
  // Janela util: o relogio de parede menos o que se gastou esperando um rAF
  // que nunca chegou.
  const elapsedMs = Math.max(1, previous - started - clock.wastedMs);
  const median = percentile(frameMs, 50);
  return {
    backend: caps.backend,
    tier: caps.tier,
    calibrateMedianMs: round(calibration.medianMs, 3),
    contextLost: calibration.contextLostDuring,
    firstFrameMs,
    stalls: clock.stalls,
    pumped: clock.pumped,
    recoveries: clock.recoveries,
    wastedMs: Math.round(clock.wastedMs),
    scene: {
      sprites: SCENE_SPRITES,
      chunks: SCENE_CHUNKS,
      median_frame_ms: round(median, 3),
      p95_frame_ms: round(percentile(frameMs, 95), 3),
      cpu_ms: round(percentile(cpuMs, 50), 3),
      avg_frame_ms: frames > 0 ? round(elapsedMs / frames, 3) : 0,
      fps: elapsedMs > 0 && frames > 0 ? round((frames / elapsedMs) * 1000, 1) : 0,
      draw_calls: drawCalls,
      frames,
    },
  };
}

/**
 * Entry point called by `src/routes/world/+page.svelte` when the URL carries
 * `?bench=<scenario>`. Always ends in a `world_bench_report` invoke, even when
 * everything else failed — a job with no JSON is a job that learned nothing.
 */
/**
 * The runner's own deadline, comfortably inside the process one
 * (`OMNIGET_WORLD_BENCH_TIMEOUT_MS`, 120 s). Whoever hits their deadline first
 * decides what the job learns, and a report that says *where* it got stuck is
 * worth more than a bare `{"error":"timeout"}` from the process side.
 */
export const REPORT_DEADLINE_MS = 90_000;

export async function runWorldBench(canvas: HTMLCanvasElement, scenario: string): Promise<void> {
  const report: WorldBenchReport = {
    scenario,
    backend: "none",
    tier: 0,
    calibrate_median_ms: 0,
    context_lost_during_calibration: false,
    scene: { ...EMPTY_SCENE },
    first_frame_ms: 0,
    raf_stalls: 0,
    raf_pumped: false,
    raf_recoveries: 0,
    raf_wasted_ms: 0,
    webgl2: probeWebgl2(),
    ipc: {
      "2kb_p50_ms": 0,
      "50kb_p50_ms": 0,
      "200kb_p50_ms": 0,
      "200kb_15hz_cpu_pct": 0,
      burst_received: 0,
      burst_sent: 0,
      binary: false,
    },
  };

  // Where the run is, so a deadline can name the step that hung.
  let stage = "probe";
  const deadline = new Promise<"deadline">((resolve) =>
    setTimeout(() => resolve("deadline"), REPORT_DEADLINE_MS),
  );
  const raced = async <T>(work: Promise<T>): Promise<T | "deadline"> => Promise.race([work, deadline]);

  try {
    stage = "import";
    const render = await import("$lib/world/render");
    stage = "scene";
    const result = await raced(runScene(canvas, render));
    if (result === "deadline") throw new Error(`ERR_WORLD_BENCH_DEADLINE:${stage}`);
    report.backend = result.backend;
    report.tier = result.tier;
    report.calibrate_median_ms = result.calibrateMedianMs;
    report.context_lost_during_calibration = result.contextLost;
    report.scene = result.scene;
    report.first_frame_ms = result.firstFrameMs;
    report.raf_stalls = result.stalls;
    report.raf_pumped = result.pumped;
    report.raf_recoveries = result.recoveries;
    report.raf_wasted_ms = result.wastedMs;
  } catch (e) {
    report.error = e instanceof Error ? e.message : String(e);
  }

  // The IPC bench does not touch the GPU, so it runs even when the renderer
  // failed: I-5 and I-12 are separate unknowns and one must not hide the other.
  stage = "ipc";
  const ipc = await raced(runIpcBench());
  if (ipc === "deadline") {
    report.ipc.error = `ERR_WORLD_BENCH_DEADLINE:${stage}`;
  } else {
    report.ipc = ipc;
  }

  // The channel here exists only because the same command multiplexes the IPC
  // ops; it never receives anything.
  await invoke("world_bench_report", { report, channel: new Channel<unknown>() });
}
