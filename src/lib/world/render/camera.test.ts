import { describe, expect, it } from 'vitest';
import {
  chunkId,
  clampZoom,
  depth,
  makeCamera,
  parseChunkId,
  project,
  screenToWorld,
  visibleBounds,
  visibleChunks,
  worldToScreen,
} from './camera';
import { TILE_H, TILE_W, TILE_Z } from './types';

describe('project', () => {
  it('maps the tile grid onto a 2:1 diamond', () => {
    expect(project(0, 0)).toEqual({ px: 0, py: 0 });
    expect(project(1, 0)).toEqual({ px: TILE_W / 2, py: TILE_H / 2 });
    expect(project(0, 1)).toEqual({ px: -TILE_W / 2, py: TILE_H / 2 });
    expect(project(1, 1)).toEqual({ px: 0, py: TILE_H });
  });

  it('raises the point by TILE_Z per z unit', () => {
    expect(project(0, 0, 1).py).toBe(-TILE_Z);
  });
});

describe('worldToScreen / screenToWorld', () => {
  const cam = makeCamera(800, 600, 4, 4, 1);

  it('puts the camera target in the middle of the viewport', () => {
    expect(worldToScreen(cam, 4, 4)).toEqual({ sx: 400, sy: 300 });
  });

  it('round-trips through the inverse on the ground plane', () => {
    for (const [x, y] of [
      [0, 0],
      [3.5, 7.25],
      [-2, 9],
    ]) {
      const s = worldToScreen(cam, x, y);
      const w = screenToWorld(cam, s.sx, s.sy);
      expect(w.x).toBeCloseTo(x, 6);
      expect(w.y).toBeCloseTo(y, 6);
    }
  });

  it('scales with zoom around the camera target', () => {
    const zoomed = makeCamera(800, 600, 4, 4, 2);
    const a = worldToScreen(cam, 6, 4);
    const b = worldToScreen(zoomed, 6, 4);
    expect(b.sx - 400).toBeCloseTo((a.sx - 400) * 2, 6);
  });
});

describe('depth', () => {
  it('is the isometric depth axis plus the elevation', () => {
    expect(depth(1, 2, 0)).toBe(3);
    expect(depth(1, 2, 1)).toBe(4);
    expect(depth(0, 0, 0)).toBe(0);
  });
});

describe('visibleBounds / visibleChunks', () => {
  it('covers the four screen corners with a margin', () => {
    const cam = makeCamera(400, 300, 0, 0, 1);
    const b = visibleBounds(cam, 2);
    const corner = screenToWorld(cam, 0, 0);
    expect(b.minX).toBeLessThanOrEqual(corner.x);
    expect(b.maxY).toBeGreaterThanOrEqual(corner.y);
  });

  it('returns chunks sorted back to front', () => {
    const cam = makeCamera(960, 540, 16, 16, 1);
    const ids = visibleChunks(cam, 0);
    expect(ids.length).toBeGreaterThan(0);
    const keys = ids.map((id) => {
      const { cx, cy } = parseChunkId(id);
      return cx + cy;
    });
    expect(keys).toEqual([...keys].sort((a, b) => a - b));
  });
});

describe('chunk ids', () => {
  it('round-trips, negatives included', () => {
    expect(chunkId(2, -3)).toBe('2,-3');
    expect(parseChunkId('2,-3')).toEqual({ cx: 2, cy: -3 });
    expect(parseChunkId(chunkId(-11, 40))).toEqual({ cx: -11, cy: 40 });
  });
});

describe('clampZoom', () => {
  it('stays between 0.25 and 4', () => {
    expect(clampZoom(0.01)).toBe(0.25);
    expect(clampZoom(10)).toBe(4);
    expect(clampZoom(1.5)).toBe(1.5);
  });
});
