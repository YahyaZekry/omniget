// Position interpolation between two simulation ticks.
//
// The simulation is authoritative and discrete: it moves an agent once every
// 100 ms and sends the result. The screen draws at 30-60 Hz. Everything
// between the two lives here, and nothing else in the route is allowed to
// invent a position.
//
// Decision 5 of the orchestration §4.4 in one sentence: the sprite clock runs
// on its own 8-12 fps, only the position is interpolated per frame.
//
// Pure module: no DOM, no clock of its own. Every function takes `nowMs`.

import { FIXED_ONE } from './codec';

/** One simulation tick in milliseconds (`TICK_MS` in the crate). */
export const TICK_MS = 100;

/**
 * How long a track is stretched over. The bridge sends at 10 Hz while the
 * route is visible, so one tick is the common case; a late blob must not make
 * an agent teleport, and an early one must not make it stutter, so the
 * duration is the measured arrival gap clamped to this window.
 */
export const MIN_TRACK_MS = 60;
export const MAX_TRACK_MS = 320;

/** Past this gap the position is snapped: the world was asleep, not lagging. */
export const SNAP_AFTER_MS = 1000;

export interface Track {
  /** Where the sprite was when the last blob arrived, in tiles. */
  fromX: number;
  fromY: number;
  fromZ: number;
  /** Where the simulation says it is now, in tiles. */
  toX: number;
  toY: number;
  toZ: number;
  startedAtMs: number;
  durationMs: number;
}

export interface Point {
  x: number;
  y: number;
  z: number;
}

/** Sub-unit coordinate (1/256 of a tile) to tiles. */
export function toTiles(fixed: number): number {
  return fixed / FIXED_ONE;
}

export function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

export function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/** A track that does not move: the starting state of every agent. */
export function still(x: number, y: number, z: number, nowMs: number): Track {
  return {
    fromX: x,
    fromY: y,
    fromZ: z,
    toX: x,
    toY: y,
    toZ: z,
    startedAtMs: nowMs,
    durationMs: MIN_TRACK_MS,
  };
}

/**
 * Point a new target. The track restarts from wherever the sprite is being
 * drawn right now, not from the previous target, so a blob that arrives late
 * never makes the sprite jump backwards.
 */
export function retarget(
  track: Track,
  x: number,
  y: number,
  z: number,
  nowMs: number,
  gapMs: number,
): void {
  if (gapMs >= SNAP_AFTER_MS) {
    track.fromX = x;
    track.fromY = y;
    track.fromZ = z;
  } else {
    const p = sample(track, nowMs);
    track.fromX = p.x;
    track.fromY = p.y;
    track.fromZ = p.z;
  }
  track.toX = x;
  track.toY = y;
  track.toZ = z;
  track.startedAtMs = nowMs;
  track.durationMs = clampTrackMs(gapMs);
}

export function clampTrackMs(gapMs: number): number {
  if (!Number.isFinite(gapMs) || gapMs <= 0) return TICK_MS;
  return Math.min(MAX_TRACK_MS, Math.max(MIN_TRACK_MS, gapMs));
}

/** Where to draw at `nowMs`. Never extrapolates: after the track it holds. */
export function sample(track: Track, nowMs: number, out?: Point): Point {
  const t = clamp01((nowMs - track.startedAtMs) / track.durationMs);
  const p = out ?? { x: 0, y: 0, z: 0 };
  p.x = lerp(track.fromX, track.toX, t);
  p.y = lerp(track.fromY, track.toY, t);
  p.z = lerp(track.fromZ, track.toZ, t);
  return p;
}

/** True while the track still has ground to cover — the HUD hides some things then. */
export function moving(track: Track, nowMs: number): boolean {
  if (track.fromX === track.toX && track.fromY === track.toY && track.fromZ === track.toZ) {
    return false;
  }
  return nowMs - track.startedAtMs < track.durationMs;
}

/**
 * Exponential mean of the gap between blobs. The bridge changes rate with the
 * sleep state (10 Hz visible, 0.2 Hz closed), so the interpolation duration is
 * measured rather than assumed — and one late blob moves the mean a little,
 * not the whole way.
 */
export class ArrivalClock {
  private last = 0;
  private mean = TICK_MS;

  /** Returns the gap this blob arrived after; 0 for the first one. */
  mark(nowMs: number): number {
    if (this.last === 0) {
      this.last = nowMs;
      return 0;
    }
    const gap = nowMs - this.last;
    this.last = nowMs;
    if (gap > 0 && gap < SNAP_AFTER_MS) this.mean = this.mean * 0.7 + gap * 0.3;
    return gap;
  }

  get meanMs(): number {
    return this.mean;
  }

  /** The duration a track started now should use. */
  get trackMs(): number {
    return clampTrackMs(this.mean);
  }

  reset(): void {
    this.last = 0;
    this.mean = TICK_MS;
  }
}
