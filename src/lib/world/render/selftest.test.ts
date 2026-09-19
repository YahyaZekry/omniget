import { describe, expect, it } from 'vitest';
import { summariseFrames, SELFTEST_LOG_PREFIX } from './selftest';
import type { FrameStats } from './types';

const last: FrameStats = { cpuMs: 1, drawCalls: 11, sprites: 64, chunksBaked: 0 };
const shape = { sprites: 64, chunks: 9, texts: 32 };

describe('summariseFrames', () => {
  it('takes CPU per frame from the wall clock, not from the clamped samples', () => {
    // 120 frames whose per-frame clock only ever reports 0 or 1 ms.
    const samples = Array.from({ length: 120 }, (_, i) => (i % 8 === 0 ? 1 : 0));
    const r = summariseFrames(samples, 8, shape, last);
    expect(r.frames).toBe(120);
    expect(r.wall_ms).toBe(8);
    expect(r.cpu_per_frame_ms).toBeCloseTo(0.067, 3);
    expect(r.cpu_median_ms).toBe(0);
    expect(r.draw_calls).toBe(11);
    expect(r.sprites_drawn).toBe(64);
  });

  it('reports the p95 of the samples it was given', () => {
    const samples = Array.from({ length: 100 }, (_, i) => i + 1);
    expect(summariseFrames(samples, 100, shape, last).cpu_p95_ms).toBe(95);
  });

  it('survives an empty run instead of dividing by zero', () => {
    const r = summariseFrames([], 0, shape, last);
    expect(r.frames).toBe(0);
    expect(r.cpu_per_frame_ms).toBe(0);
    expect(r.cpu_median_ms).toBe(0);
  });

  it('carries the scene shape through untouched', () => {
    const r = summariseFrames([1], 1, shape, last);
    expect([r.sprites, r.chunks, r.texts]).toEqual([64, 9, 32]);
  });

  it('keeps a grep-able prefix for the log line', () => {
    expect(SELFTEST_LOG_PREFIX).toBe('[world-selftest]');
  });
});
