// Reproducible measurement of the renderer, on the machine that runs it.
// Reachable from the hidden route as `/world?bench=selftest`; it does not depend
// on the bench runner of `f6-bench`, so the two can be measured separately.
//
// What it produces, in one JSON object:
//   - the I-4 table: readPixels vs gl.finish() vs EXT_disjoint_timer_query_webgl2
//     on the calibration scene, plus the tier the measurement lands on;
//   - the synthetic scene (64 sprites, 9 chunks, 32 texts) over N frames, with
//     CPU per frame taken from WALL time of the whole run divided by the frames
//     (WKWebView clamps performance.now() to 1 ms, so per-frame medians saturate)
//     and the draw calls of the last frame;
//   - the sleep proof: rAF callbacks counted while the renderer sleeps;
//   - the wake proof: one frame rendered after wake().
//
// The frame loop is synchronous on purpose: an occluded webview throttles (and
// on macOS suspends) timers and rAF, which would turn a 120-frame measurement
// into minutes of nothing. "CPU de render" is defined as the JS time from the
// start of the frame to the last GL call, which is exactly what this measures.

import { calibrateDetailed, type CalibrationDetail } from './calibrate';
import { attachBenchTexts, buildBenchScene, createSyntheticAtlas, primeBenchChunks } from './bench-scene';
import { createRenderer, createLoop, getRafTicks, resetRafTicks } from './index';
import type { FrameStats, Tier } from './types';

export const SELFTEST_LOG_PREFIX = '[world-selftest]';

export interface SelftestOpts {
  sprites?: number;
  chunks?: number;
  texts?: number;
  /** Measured frames, after the warm-up. */
  frames?: number;
  warmup?: number;
  /** How long the renderer stays asleep while rAF callbacks are counted. */
  hiddenMs?: number;
  /** Forces a tier instead of using the measured one. */
  tier?: Tier;
}

export interface SelftestSceneReport {
  sprites: number;
  chunks: number;
  texts: number;
  frames: number;
  /** Wall time of the measured frames. */
  wall_ms: number;
  /** wall_ms / frames — the number that answers "CPU de render por frame". */
  cpu_per_frame_ms: number;
  /** Median of the per-frame FrameStats.cpuMs, kept to show the clock's floor. */
  cpu_median_ms: number;
  cpu_p95_ms: number;
  draw_calls: number;
  sprites_drawn: number;
  chunks_baked: number;
}

export interface SelftestReport {
  kind: 'world-selftest';
  scenario: string;
  backend: string;
  tier: Tier;
  dpr: number;
  max_texture_size: number;
  timer_query: boolean;
  calibration: CalibrationDetail | null;
  first_frame: FrameStats | null;
  first_frame_wall_ms: number;
  scene: SelftestSceneReport | null;
  sleep: { raf_before: number; raf_during: number; hidden_ms: number } | null;
  after_wake: FrameStats | null;
  error?: string;
}

/** Pure: turns the per-frame samples and the wall clock into the scene report. */
export function summariseFrames(
  samples: readonly number[],
  wallMs: number,
  shape: { sprites: number; chunks: number; texts: number },
  last: FrameStats,
): SelftestSceneReport {
  const sorted = Array.from(samples).sort((a, b) => a - b);
  const frames = sorted.length;
  return {
    sprites: shape.sprites,
    chunks: shape.chunks,
    texts: shape.texts,
    frames,
    wall_ms: round3(wallMs),
    cpu_per_frame_ms: frames > 0 ? round3(wallMs / frames) : 0,
    cpu_median_ms: frames > 0 ? sorted[frames >> 1] : 0,
    cpu_p95_ms: frames > 0 ? sorted[Math.max(0, Math.ceil(frames * 0.95) - 1)] : 0,
    draw_calls: last.drawCalls,
    sprites_drawn: last.sprites,
    chunks_baked: last.chunksBaked,
  };
}

function round3(v: number): number {
  return Math.round(v * 1000) / 1000;
}

/**
 * Runs the whole measurement on `canvas` and resolves with the report. Never
 * throws: a failure becomes the `error` field, so the caller always has data.
 */
export async function runSelftest(
  canvas: HTMLCanvasElement,
  opts: SelftestOpts = {},
): Promise<SelftestReport> {
  const sprites = opts.sprites ?? 64;
  const chunks = opts.chunks ?? 9;
  const texts = opts.texts ?? 32;
  const frames = opts.frames ?? 120;
  const warmup = opts.warmup ?? 15;
  const hiddenMs = opts.hiddenMs ?? 3000;

  const report: SelftestReport = {
    kind: 'world-selftest',
    scenario: 'selftest',
    backend: 'none',
    tier: 0,
    dpr: 1,
    max_texture_size: 0,
    timer_query: false,
    calibration: null,
    first_frame: null,
    first_frame_wall_ms: 0,
    scene: null,
    sleep: null,
    after_wake: null,
  };

  const renderer = createRenderer();
  try {
    const calibration = await calibrateDetailed(null);
    report.calibration = calibration;
    const tier = opts.tier ?? calibration.tier;

    const caps = await renderer.init(canvas, { tier });
    report.backend = caps.backend;
    report.tier = caps.tier;
    report.dpr = caps.dpr;
    report.max_texture_size = caps.maxTextureSize;
    report.timer_query = caps.timerQuery;

    const atlas = await createSyntheticAtlas();
    renderer.loadAtlas(atlas.json, atlas.pages);
    const scene = buildBenchScene({ sprites, chunks, texts });
    primeBenchChunks(renderer, scene);
    attachBenchTexts(renderer, scene, texts);
    renderer.resize(960, 540, 1);
    scene.camera.width = 960;
    scene.camera.height = 540;

    const ff0 = performance.now();
    report.first_frame = renderer.frame(scene, 16.7);
    report.first_frame_wall_ms = round3(performance.now() - ff0);

    // Warm-up: the first frames bake the chunks, two per frame.
    for (let i = 0; i < warmup; i++) {
      step(scene.sprites, i);
      renderer.frame(scene, 16.7);
    }

    const samples: number[] = [];
    let last = report.first_frame;
    const t0 = performance.now();
    for (let i = 0; i < frames; i++) {
      step(scene.sprites, warmup + i);
      last = renderer.frame(scene, 16.7);
      samples.push(last.cpuMs);
    }
    const wall = performance.now() - t0;
    report.scene = summariseFrames(samples, wall, { sprites, chunks, texts }, last);

    // Sleep proof: the loop is started so a running world has a baseline, then
    // stopped and the renderer put to sleep with the rAF counter reset.
    const loop = createLoop(renderer, () => scene, { watchdog: false });
    loop.start();
    await wait(250);
    const rafBefore = getRafTicks();
    loop.stop();
    renderer.sleep();
    resetRafTicks();
    const hidden0 = performance.now();
    await wait(hiddenMs);
    report.sleep = {
      raf_before: rafBefore,
      raf_during: getRafTicks(),
      hidden_ms: round3(performance.now() - hidden0),
    };

    await renderer.wake();
    renderer.loadAtlas(atlas.json, atlas.pages);
    primeBenchChunks(renderer, scene);
    report.after_wake = renderer.frame(scene, 16.7);
  } catch (e) {
    report.error = e instanceof Error ? e.message : String(e);
  } finally {
    try {
      renderer.destroy();
    } catch {
      // Already gone; the report is what matters.
    }
  }
  return report;
}

/** Moves the sprites so the batcher has to re-sort every frame. */
function step(sprites: { x: number; y: number }[], i: number): void {
  for (let s = 0; s < sprites.length; s++) {
    sprites[s].x += Math.sin((i + s) / 10) * 0.01;
  }
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Prints the report as one line the CI and a human can both read, and hands it
 * to `world_bench_report` when the Tauri IPC is there (which makes the app print
 * `[world-bench] {json}` and exit). The console line is the contract; the IPC is
 * a convenience and never a requirement.
 */
export async function reportSelftest(report: SelftestReport): Promise<void> {
  const line = `${SELFTEST_LOG_PREFIX} ${JSON.stringify(report)}`;
  console.log(line);
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('world_bench_report', { report });
  } catch {
    // Outside Tauri, or the command is not registered: the console line stands.
  }
}
