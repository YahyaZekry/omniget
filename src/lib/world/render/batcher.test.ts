import { describe, expect, it } from 'vitest';
import {
  buildIndices,
  buildSpriteBatches,
  cullSprites,
  drawCalls,
  growScratch,
  sortByDepth,
  spriteDepth,
  unpackTint,
  writeQuad,
  FLOATS_PER_QUAD,
  INDICES_PER_QUAD,
} from './batcher';
import { parseAtlas } from './atlas';
import { makeCamera } from './camera';
import type { SpriteInst } from './types';

const atlasJson = {
  version: 1,
  pages: ['p-0.png', 'p-1.png'],
  frames: {
    a: { page: 0, x: 0, y: 0, w: 32, h: 48, pivot: [16, 46] },
    b: { page: 1, x: 8, y: 8, w: 16, h: 16, pivot: [8, 16] },
    tall: { page: 0, x: 64, y: 0, w: 32, h: 64, pivot: [16, 60], z_base: 32 },
  },
};
const atlas = parseAtlas(atlasJson);
const sizes = [
  { w: 256, h: 256 },
  { w: 128, h: 128 },
];
const cam = makeCamera(800, 600, 0, 0, 1);

function sprite(p: Partial<SpriteInst>): SpriteInst {
  return { frame: 'a', x: 0, y: 0, z: 0, ...p };
}

describe('unpackTint', () => {
  it('splits 0xRRGGBB into normalised channels', () => {
    expect(unpackTint(0xffffff)).toEqual([1, 1, 1]);
    expect(unpackTint(0x000000)).toEqual([0, 0, 0]);
    const [r, g, b] = unpackTint(0xff8000);
    expect(r).toBe(1);
    expect(g).toBeCloseTo(128 / 255, 5);
    expect(b).toBe(0);
  });
});

describe('writeQuad', () => {
  it('writes 32 floats in four vertices, clockwise from top-left', () => {
    const out = new Float32Array(FLOATS_PER_QUAD);
    const next = writeQuad(out, 0, {
      page: 0,
      sx: 10,
      sy: 20,
      w: 30,
      h: 40,
      u0: 0,
      v0: 0,
      u1: 1,
      v1: 1,
    });
    expect(next).toBe(FLOATS_PER_QUAD);
    expect([out[0], out[1]]).toEqual([10, 20]);
    expect([out[8], out[9]]).toEqual([40, 20]);
    expect([out[16], out[17]]).toEqual([40, 60]);
    expect([out[24], out[25]]).toEqual([10, 60]);
  });

  it('mirrors by swapping U, never by touching the geometry', () => {
    const plain = new Float32Array(FLOATS_PER_QUAD);
    const flipped = new Float32Array(FLOATS_PER_QUAD);
    const q = { page: 0, sx: 0, sy: 0, w: 10, h: 10, u0: 0.1, v0: 0.2, u1: 0.5, v1: 0.6 };
    writeQuad(plain, 0, q);
    writeQuad(flipped, 0, { ...q, flip: true });
    expect(plain[2]).toBeCloseTo(0.1, 6);
    expect(flipped[2]).toBeCloseTo(0.5, 6);
    expect(flipped[10]).toBeCloseTo(0.1, 6);
    // Positions identical.
    expect(Array.from(flipped.filter((_, i) => i % 8 < 2))).toEqual(
      Array.from(plain.filter((_, i) => i % 8 < 2)),
    );
  });

  it('carries tint and alpha into every vertex', () => {
    const out = new Float32Array(FLOATS_PER_QUAD);
    writeQuad(out, 0, {
      page: 0,
      sx: 0,
      sy: 0,
      w: 1,
      h: 1,
      u0: 0,
      v0: 0,
      u1: 1,
      v1: 1,
      tint: 0x00ff00,
      alpha: 0.5,
    });
    for (let v = 0; v < 4; v++) {
      expect(out[v * 8 + 4]).toBe(0);
      expect(out[v * 8 + 5]).toBe(1);
      expect(out[v * 8 + 7]).toBe(0.5);
    }
  });
});

describe('buildIndices', () => {
  it('emits two triangles per quad', () => {
    const idx = buildIndices(2);
    expect(idx.length).toBe(2 * INDICES_PER_QUAD);
    expect(Array.from(idx.slice(0, 6))).toEqual([0, 1, 2, 0, 2, 3]);
    expect(Array.from(idx.slice(6))).toEqual([4, 5, 6, 4, 6, 7]);
  });
});

describe('depth sort', () => {
  it('sorts back to front by ground depth plus z', () => {
    const sprites = [
      sprite({ x: 5, y: 5 }),
      sprite({ x: 0, y: 0 }),
      sprite({ x: 2, y: 1 }),
    ];
    expect(sortByDepth(sprites, atlas)).toEqual([1, 2, 0]);
  });

  it('puts a higher sprite in front of one on the same tile', () => {
    const sprites = [sprite({ x: 3, y: 3, z: 2 }), sprite({ x: 3, y: 3, z: 0 })];
    expect(sortByDepth(sprites, atlas)).toEqual([1, 0]);
  });

  it('is stable for equal keys', () => {
    const sprites = [sprite({ x: 1, y: 1 }), sprite({ x: 2, y: 0 }), sprite({ x: 0, y: 2 })];
    expect(sortByDepth(sprites, atlas)).toEqual([0, 1, 2]);
  });

  it('adds the frame z_base', () => {
    const flat = sprite({ frame: 'a', x: 1, y: 1 });
    const tall = sprite({ frame: 'tall', x: 1, y: 1 });
    expect(spriteDepth(tall, atlas.frames.get('tall'))).toBeGreaterThan(
      spriteDepth(flat, atlas.frames.get('a')),
    );
  });
});

describe('buildSpriteBatches', () => {
  it('produces one quad per sprite and one batch per page run', () => {
    const sprites = [
      sprite({ frame: 'a', x: 0, y: 0 }),
      sprite({ frame: 'a', x: 1, y: 0 }),
      sprite({ frame: 'b', x: 2, y: 0 }),
    ];
    const r = buildSpriteBatches(sprites, atlas, cam, sizes);
    expect(r.quads).toBe(3);
    expect(r.skipped).toBe(0);
    expect(r.batches.length).toBe(2);
    expect(r.batches[0]).toEqual({ page: 0, offset: 0, count: 2 * INDICES_PER_QUAD });
    expect(r.batches[1].page).toBe(1);
    expect(r.batches[1].offset).toBe(2 * INDICES_PER_QUAD);
  });

  it('breaks a batch when the page alternates along the depth order', () => {
    const sprites = [
      sprite({ frame: 'a', x: 0, y: 0 }),
      sprite({ frame: 'b', x: 1, y: 0 }),
      sprite({ frame: 'a', x: 2, y: 0 }),
    ];
    const r = buildSpriteBatches(sprites, atlas, cam, sizes);
    expect(r.batches.map((b) => b.page)).toEqual([0, 1, 0]);
    expect(drawCalls(r.batches, 9)).toBe(12);
  });

  it('skips unknown frames instead of throwing', () => {
    const r = buildSpriteBatches([sprite({ frame: 'nope' })], atlas, cam, sizes);
    expect(r.quads).toBe(0);
    expect(r.skipped).toBe(1);
    expect(r.batches.length).toBe(0);
  });

  it('places the pivot on the ground point and mirrors the pivot with the sprite', () => {
    const plain = buildSpriteBatches([sprite({ frame: 'a' })], atlas, cam, sizes);
    const flipped = buildSpriteBatches([sprite({ frame: 'a', flip: true })], atlas, cam, sizes);
    // pivot x 16 of a 32 px frame is the centre, so mirroring keeps it in place.
    expect(flipped.vertices[0]).toBeCloseTo(plain.vertices[0], 6);
    expect(plain.vertices[1]).toBeCloseTo(600 / 2 - 46, 6);
  });

  it('reuses the scratch buffer when it is big enough', () => {
    const scratch = new Float32Array(FLOATS_PER_QUAD * 4);
    const r = buildSpriteBatches([sprite({})], atlas, cam, sizes, scratch);
    expect(r.vertices).toBe(scratch);
  });
});

describe('cullSprites', () => {
  it('drops sprites off screen and keeps the ones inside', () => {
    const inside = sprite({ x: 0, y: 0 });
    const far = sprite({ x: 400, y: 400 });
    const kept = cullSprites([inside, far], atlas, cam);
    expect(kept).toEqual([inside]);
  });
});

describe('growScratch', () => {
  it('grows in powers of two and keeps a big enough buffer', () => {
    const a = growScratch(null, 100);
    expect(a.length).toBe(1024);
    expect(growScratch(a, 500)).toBe(a);
    expect(growScratch(a, 5000).length).toBe(8192);
  });
});
