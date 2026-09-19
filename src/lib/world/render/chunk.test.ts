import { describe, expect, it } from 'vitest';
import {
  chunkLayout,
  chunkTextureBytes,
  chunkTextureCap,
  chunkTilePx,
  sortTiles,
  ChunkStore,
} from './chunk';
import { ShelfPacker, Lru, cacheKey, styleKey, textUV } from './text';
import { benchChunkIds, buildBenchScene, benchChunkTiles, benchTextLabels } from './bench-scene';
import { CHUNK_TILES } from './types';

describe('chunk layout', () => {
  it('puts the diamond in the bottom half, leaving room for elevation', () => {
    const l = chunkLayout(512);
    expect(l.scale).toBeCloseTo(512 / (CHUNK_TILES * 64), 6);
    expect(l.originX).toBe(256);
    expect(l.originY).toBe(256);
    const top = chunkTilePx(l, 0, 0);
    const bottom = chunkTilePx(l, CHUNK_TILES, CHUNK_TILES);
    expect(top.py).toBe(256);
    expect(bottom.py).toBeCloseTo(512, 6);
    expect(chunkTilePx(l, 0, 0, 1).py).toBeLessThan(top.py);
  });

  it('keeps every tile of a full chunk inside the texture', () => {
    const l = chunkLayout(256);
    for (const t of benchChunkTiles()) {
      const p = chunkTilePx(l, t.lx, t.ly, t.z ?? 0);
      expect(p.px).toBeGreaterThanOrEqual(0);
      expect(p.px).toBeLessThanOrEqual(256);
      expect(p.py).toBeGreaterThanOrEqual(0);
      expect(p.py).toBeLessThanOrEqual(256);
    }
  });
});

describe('sortTiles', () => {
  it('sorts back to front and does not mutate the input', () => {
    const tiles = [
      { frame: 'a', lx: 3, ly: 3 },
      { frame: 'a', lx: 0, ly: 0 },
      { frame: 'a', lx: 0, ly: 0, z: 1 },
    ];
    const sorted = sortTiles(tiles);
    expect(sorted.map((t) => t.lx + t.ly + (t.z ?? 0))).toEqual([0, 1, 6]);
    expect(tiles[0].lx).toBe(3);
  });
});

describe('ChunkStore', () => {
  it('marks a new chunk dirty and clears it after a bake', () => {
    const s = new ChunkStore();
    s.set('0,0', [{ frame: 'a', lx: 0, ly: 0 }]);
    expect(s.dirtyAmong(['0,0'])).toEqual(['0,0']);
    s.markBaked('0,0');
    expect(s.dirtyAmong(['0,0'])).toEqual([]);
    s.invalidate('0,0');
    expect(s.dirtyAmong(['0,0'])).toEqual(['0,0']);
  });

  it('caps how many chunks a single frame bakes', () => {
    const s = new ChunkStore();
    const ids = ['0,0', '1,0', '2,0', '3,0'];
    for (const id of ids) s.set(id, []);
    expect(s.dirtyAmong(ids, 2).length).toBe(2);
    expect(s.dirtyAmong(ids, 10).length).toBe(4);
  });

  it('only reports the visible dirty chunks', () => {
    const s = new ChunkStore();
    s.set('0,0', []);
    s.set('9,9', []);
    expect(s.dirtyAmong(['0,0'], 4)).toEqual(['0,0']);
  });

  it('evicts the least recently used beyond the cap', () => {
    const s = new ChunkStore();
    s.set('0,0', []);
    s.set('1,0', []);
    s.set('2,0', []);
    s.touch(['1,0']);
    s.touch(['2,0']);
    expect(s.evictable(2)).toEqual(['0,0']);
    expect(s.evictable(5)).toEqual([]);
  });

  it('invalidateAll marks every chunk after a context loss', () => {
    const s = new ChunkStore();
    s.set('0,0', []);
    s.markBaked('0,0');
    s.invalidateAll();
    expect(s.dirtyAmong(['0,0'])).toEqual(['0,0']);
  });
});

describe('chunk texture budget', () => {
  it('counts RGBA8 and fits the tier budget', () => {
    expect(chunkTextureBytes(512, 4)).toBe(512 * 512 * 4 * 4);
    expect(chunkTextureCap(512, 64 * 1024 * 1024)).toBe(64);
    expect(chunkTextureCap(1024, Infinity)).toBe(64);
    expect(chunkTextureCap(1024, 1024)).toBe(4);
  });
});

describe('ShelfPacker', () => {
  it('fills a row then opens the next one', () => {
    const p = new ShelfPacker(100, 100);
    expect(p.alloc(60, 20)).toEqual({ x: 0, y: 0, w: 60, h: 20 });
    expect(p.alloc(30, 10)).toEqual({ x: 60, y: 0, w: 30, h: 10 });
    expect(p.alloc(50, 10)).toEqual({ x: 0, y: 20, w: 50, h: 10 });
  });

  it('refuses what does not fit the page', () => {
    const p = new ShelfPacker(32, 32);
    expect(p.alloc(40, 10)).toBeNull();
    p.alloc(32, 32);
    expect(p.alloc(1, 1)).toBeNull();
    p.reset();
    expect(p.alloc(1, 1)).not.toBeNull();
  });
});

describe('Lru', () => {
  it('evicts the least recently used and reports it', () => {
    const evicted: string[] = [];
    const lru = new Lru<number>(2, (k) => evicted.push(k));
    lru.set('a', 1);
    lru.set('b', 2);
    lru.get('a');
    lru.set('c', 3);
    expect(evicted).toEqual(['b']);
    expect(lru.keys()).toEqual(['a', 'c']);
    expect(lru.size).toBe(2);
  });
});

describe('text keys and uv', () => {
  it('keys by style and string, and normalises the rect', () => {
    expect(styleKey({ size: 12 })).not.toBe(styleKey({ size: 14 }));
    expect(cacheKey('a', { size: 12 })).not.toBe(cacheKey('b', { size: 12 }));
    expect(textUV({ x: 0, y: 512, w: 256, h: 32 }, 1024)).toEqual({
      u0: 0,
      v0: 0.5,
      u1: 0.25,
      v1: 0.53125,
    });
  });
});

describe('bench scene', () => {
  it('is deterministic and sized by the options', () => {
    const a = buildBenchScene({ sprites: 64, chunks: 9, texts: 32 });
    const b = buildBenchScene({ sprites: 64, chunks: 9, texts: 32 });
    expect(a.sprites.length).toBe(64);
    expect(a.chunksVisible.length).toBe(9);
    expect(a.sprites[7]).toEqual(b.sprites[7]);
    expect(benchChunkIds(9)).toEqual([
      '0,0',
      '1,0',
      '2,0',
      '0,1',
      '1,1',
      '2,1',
      '0,2',
      '1,2',
      '2,2',
    ]);
    expect(benchTextLabels(3)).toEqual(['agent-00', 'agent-01', 'agent-02']);
  });

  it('fills a chunk with floor and two walls', () => {
    const tiles = benchChunkTiles();
    expect(tiles.filter((t) => t.frame === 'bench/floor').length).toBe(CHUNK_TILES * CHUNK_TILES);
    expect(tiles.length).toBeGreaterThan(CHUNK_TILES * CHUNK_TILES);
  });
});
