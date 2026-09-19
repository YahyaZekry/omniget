import { describe, expect, it } from 'vitest';
import {
  ArrivalClock,
  MAX_TRACK_MS,
  MIN_TRACK_MS,
  SNAP_AFTER_MS,
  TICK_MS,
  clamp01,
  clampTrackMs,
  lerp,
  moving,
  retarget,
  sample,
  still,
  toTiles,
} from './interp';

describe('interp: fixed point', () => {
  it('256 sub-units are one tile', () => {
    expect(toTiles(256)).toBe(1);
    expect(toTiles(2176)).toBe(8.5);
    expect(toTiles(-128)).toBe(-0.5);
  });

  it('clamps and interpolates', () => {
    expect(clamp01(-1)).toBe(0);
    expect(clamp01(2)).toBe(1);
    expect(lerp(0, 10, 0.25)).toBe(2.5);
  });
});

describe('interp: tracks', () => {
  it('a still track samples to its own point forever', () => {
    const track = still(3, 4, 0, 1000);
    expect(sample(track, 1000)).toEqual({ x: 3, y: 4, z: 0 });
    expect(sample(track, 99_000)).toEqual({ x: 3, y: 4, z: 0 });
    expect(moving(track, 1000)).toBe(false);
  });

  it('moves from where it is to where the simulation says, then holds', () => {
    const track = still(0, 0, 0, 0);
    retarget(track, 1, 0, 0, 0, TICK_MS);
    expect(sample(track, 0).x).toBe(0);
    expect(sample(track, 50).x).toBeCloseTo(0.5, 6);
    expect(sample(track, 100).x).toBe(1);
    // Never extrapolates past the target, whatever the frame clock does.
    expect(sample(track, 5000).x).toBe(1);
  });

  it('a new target mid-flight starts from the drawn position, not the old one', () => {
    const track = still(0, 0, 0, 0);
    retarget(track, 2, 0, 0, 0, TICK_MS);
    // Halfway there, the simulation changes its mind.
    retarget(track, 0, 0, 0, 50, TICK_MS);
    expect(track.fromX).toBeCloseTo(1, 6);
    expect(sample(track, 50).x).toBeCloseTo(1, 6);
    expect(sample(track, 150).x).toBe(0);
  });

  it('a blob after a long silence snaps instead of gliding across the house', () => {
    const track = still(0, 0, 0, 0);
    retarget(track, 12, 9, 0, SNAP_AFTER_MS + 1, SNAP_AFTER_MS + 1);
    expect(sample(track, SNAP_AFTER_MS + 1)).toEqual({ x: 12, y: 9, z: 0 });
  });

  it('the track duration is clamped both ways', () => {
    expect(clampTrackMs(0)).toBe(TICK_MS);
    expect(clampTrackMs(Number.NaN)).toBe(TICK_MS);
    expect(clampTrackMs(10)).toBe(MIN_TRACK_MS);
    expect(clampTrackMs(5000)).toBe(MAX_TRACK_MS);
    expect(clampTrackMs(120)).toBe(120);
  });
});

describe('interp: the arrival clock', () => {
  it('starts at one tick and follows the measured rate', () => {
    const clock = new ArrivalClock();
    expect(clock.meanMs).toBe(TICK_MS);
    expect(clock.mark(1000)).toBe(0);
    for (let i = 1; i <= 20; i++) clock.mark(1000 + i * 200);
    expect(clock.meanMs).toBeGreaterThan(180);
    expect(clock.meanMs).toBeLessThanOrEqual(200);
    expect(clock.trackMs).toBeLessThanOrEqual(MAX_TRACK_MS);
  });

  it('one very late blob does not drag the mean with it', () => {
    const clock = new ArrivalClock();
    clock.mark(0);
    for (let i = 1; i <= 10; i++) clock.mark(i * 100);
    const before = clock.meanMs;
    clock.mark(10_000);
    expect(clock.meanMs).toBe(before);
  });

  it('reset puts it back to the tick rate', () => {
    const clock = new ArrivalClock();
    clock.mark(0);
    clock.mark(300);
    clock.reset();
    expect(clock.meanMs).toBe(TICK_MS);
    expect(clock.mark(5000)).toBe(0);
  });
});
