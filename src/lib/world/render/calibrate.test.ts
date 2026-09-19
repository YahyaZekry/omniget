import { describe, expect, it } from 'vitest';
import {
  buildCalibrationTexture,
  buildCalibrationVertices,
  calibrate,
  calibrateDetailed,
  median,
  runPass,
  CAL_CHUNK_QUADS,
  CAL_FRAMES,
  CAL_SPRITE_QUADS,
  CAL_TEXTURE_SIZE,
  CAL_TEXT_QUADS,
  CAL_WARMUP,
} from './calibrate';
import { getGlContext, GL_ATTRIBUTES } from './gl2';
import { FLOATS_PER_QUAD } from './batcher';
import { tierFromMedian } from './tier';

describe('median', () => {
  it('handles odd, even and empty inputs', () => {
    expect(median([5, 1, 3])).toBe(3);
    expect(median([4, 1, 3, 2])).toBe(2.5);
    expect(median([])).toBe(0);
  });
});

describe('runPass', () => {
  it('drops the warm-up frames and measures the rest', () => {
    let clock = 0;
    let draws = 0;
    let syncs = 0;
    const samples = runPass(
      () => {
        draws++;
        clock += 3;
      },
      () => {
        syncs++;
        clock += 1;
      },
      () => clock,
      5,
      2,
    );
    expect(samples).toEqual([4, 4, 4, 4, 4]);
    expect(draws).toBe(7);
    expect(syncs).toBe(7);
  });

  it('is what the tier comes from: a slow fake GL lands on tier 0', () => {
    let clock = 0;
    const samples = runPass(
      () => {
        clock += 40;
      },
      () => {},
      () => clock,
      CAL_FRAMES,
      CAL_WARMUP,
    );
    expect(samples.length).toBe(CAL_FRAMES);
    expect(tierFromMedian(median(samples))).toBe(0);
  });

  it('a fast fake GL lands on tier 3', () => {
    let clock = 0;
    const samples = runPass(
      () => {
        clock += 1.5;
      },
      () => {},
      () => clock,
      CAL_FRAMES,
      CAL_WARMUP,
    );
    expect(tierFromMedian(median(samples))).toBe(3);
  });
});

describe('calibration scene', () => {
  it('is the scene the plan describes: 9 chunks, 256 sprites, 32 texts', () => {
    const { vertices, quads } = buildCalibrationVertices();
    expect(quads).toBe(CAL_CHUNK_QUADS + CAL_SPRITE_QUADS + CAL_TEXT_QUADS);
    expect(quads).toBe(297);
    expect(vertices.length).toBe(quads * FLOATS_PER_QUAD);
  });

  it('builds a deterministic 2048 texture', () => {
    const a = buildCalibrationTexture(64);
    const b = buildCalibrationTexture(64);
    expect(a.length).toBe(64 * 64 * 4);
    expect(Array.from(a.slice(0, 8))).toEqual(Array.from(b.slice(0, 8)));
    expect(CAL_TEXTURE_SIZE).toBe(2048);
  });
});

describe('getGlContext', () => {
  it('asks for webgl2 with failIfMajorPerformanceCaveat off (llvmpipe must pass)', () => {
    const asked: Array<{ id: string; attrs: unknown }> = [];
    const fakeCanvas = {
      getContext(id: string, attrs: unknown) {
        asked.push({ id, attrs });
        return id === 'webgl2' ? { fake: true } : null;
      },
    } as unknown as HTMLCanvasElement;
    expect(getGlContext(fakeCanvas, 2)).toEqual({ fake: true });
    expect(asked[0].id).toBe('webgl2');
    expect(GL_ATTRIBUTES.failIfMajorPerformanceCaveat).toBe(false);
    expect(GL_ATTRIBUTES.antialias).toBe(false);
    expect(GL_ATTRIBUTES.depth).toBe(false);
  });

  it('falls back to webgl1 when version 1 is asked for', () => {
    const fakeCanvas = {
      getContext: (id: string) => (id === 'webgl' ? { v1: true } : null),
    } as unknown as HTMLCanvasElement;
    expect(getGlContext(fakeCanvas, 1)).toEqual({ v1: true });
  });
});

describe('calibrate without a DOM', () => {
  it('reports tier 0 and backend none instead of throwing', async () => {
    const d = await calibrateDetailed(null);
    expect(d.tier).toBe(0);
    expect(d.backend).toBe('none');
    expect(d.frames).toBe(0);
    const r = await calibrate(null);
    expect(r.tier).toBe(0);
    expect(r.contextLostDuring).toBe(false);
  });
});
