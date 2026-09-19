import { describe, expect, it } from 'vitest';
import { atlasTextureBytes, frameUV, getFrame, parseAtlas, resolveDir, MIRRORED } from './atlas';
import { syntheticAtlasJson } from './bench-scene';

const base = {
  version: 1,
  pages: ['omni-0.png'],
  frames: {
    'omni/walk/S/0': { page: 0, x: 0, y: 0, w: 32, h: 48, pivot: [16, 46] },
    'omni/walk/SW/0': { page: 0, x: 32, y: 0, w: 32, h: 48, pivot: [16, 46] },
    'omni/walk/W/0': { page: 0, x: 64, y: 0, w: 32, h: 48, pivot: [16, 46] },
    'omni/walk/NW/0': { page: 0, x: 96, y: 0, w: 32, h: 48, pivot: [16, 46] },
    'omni/walk/N/0': { page: 0, x: 128, y: 0, w: 32, h: 48, pivot: [16, 46] },
    'floor/grass': { page: 0, x: 0, y: 64, w: 64, h: 32, pivot: [32, 16] },
  },
  anims: {
    'omni/walk': {
      fps: 10,
      dirs: {
        S: ['omni/walk/S/0'],
        SW: ['omni/walk/SW/0'],
        W: ['omni/walk/W/0'],
        NW: ['omni/walk/NW/0'],
        N: ['omni/walk/N/0'],
        SE: { mirror_of: 'SW' },
        E: { mirror_of: 'W' },
        NE: { mirror_of: 'NW' },
      },
    },
  },
  tiles: { 'floor/grass': { frame: 'floor/grass', height: 0 } },
};

describe('parseAtlas', () => {
  it('reads frames, anims and tiles', () => {
    const a = parseAtlas(base);
    expect(a.frames.size).toBe(6);
    expect(a.anims.size).toBe(1);
    expect(a.tiles.get('floor/grass')?.walkable).toBe(true);
    expect(a.frames.get('omni/walk/S/0')?.pivotY).toBe(46);
    expect(a.frames.get('omni/walk/S/0')?.zBase).toBe(0);
  });

  it('rejects an unknown version', () => {
    expect(() => parseAtlas({ ...base, version: 2 })).toThrow(/version/);
  });

  it('rejects a frame pointing at a page that does not exist', () => {
    const broken = { ...base, frames: { x: { page: 3, x: 0, y: 0, w: 1, h: 1, pivot: [0, 0] } } };
    expect(() => parseAtlas(broken)).toThrow(/page/);
  });

  it('rejects an anim referencing an unknown frame', () => {
    const broken = structuredClone(base) as typeof base;
    (broken.anims['omni/walk'].dirs as Record<string, unknown>).S = ['ghost'];
    expect(() => parseAtlas(broken)).toThrow(/unknown frame/);
  });

  it('rejects a mirror of the wrong direction', () => {
    const broken = structuredClone(base) as typeof base;
    (broken.anims['omni/walk'].dirs as Record<string, unknown>).E = { mirror_of: 'NW' };
    expect(() => parseAtlas(broken)).toThrow(/must mirror/);
  });

  it('keeps the mirror table of the three derived directions', () => {
    expect(MIRRORED).toEqual({ SE: 'SW', E: 'W', NE: 'NW' });
  });
});

describe('resolveDir', () => {
  const a = parseAtlas(base);

  it('returns the unique directions unflipped', () => {
    expect(resolveDir(a, 'omni/walk', 'W')).toMatchObject({
      frames: ['omni/walk/W/0'],
      flip: false,
      fps: 10,
    });
  });

  it('returns the mirrored directions flipped over the source art', () => {
    const e = resolveDir(a, 'omni/walk', 'E');
    expect(e.flip).toBe(true);
    expect(e.frames).toEqual(['omni/walk/W/0']);
    expect(resolveDir(a, 'omni/walk', 'NE').frames).toEqual(['omni/walk/NW/0']);
    expect(resolveDir(a, 'omni/walk', 'SE').frames).toEqual(['omni/walk/SW/0']);
  });

  it('throws with a stable code for an unknown anim', () => {
    expect(() => resolveDir(a, 'nope', 'S')).toThrow(/unknown anim/);
  });
});

describe('frameUV', () => {
  it('normalises the rect against the page size', () => {
    const f = getFrame(parseAtlas(base), 'floor/grass');
    expect(frameUV(f, 256, 256)).toEqual({ u0: 0, v0: 0.25, u1: 0.25, v1: 0.375 });
  });
});

describe('atlasTextureBytes', () => {
  it('counts RGBA8 without mipmaps', () => {
    expect(atlasTextureBytes([{ w: 2048, h: 2048 }])).toBe(16777216);
  });
});

describe('synthetic atlas', () => {
  it('parses against the same rules as the real one', () => {
    const a = parseAtlas(syntheticAtlasJson());
    expect(a.frames.size).toBeGreaterThanOrEqual(6);
    expect(resolveDir(a, 'bench/idle', 'E').flip).toBe(true);
  });
});
