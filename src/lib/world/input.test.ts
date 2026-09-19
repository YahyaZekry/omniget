import { describe, expect, it } from 'vitest';
import { makeCamera, screenToWorld, worldToScreen } from '$lib/world/render/camera';
import {
  DRAG_SLOP,
  KEY_STEP,
  PICK_RADIUS_TILES,
  ZOOM_STEP,
  keyPan,
  keyZoom,
  panByPixels,
  pickAgent,
  tileAt,
  zoomAt,
} from './input';

describe('input: picking', () => {
  it('the tile under the middle of the canvas is the one the camera looks at', () => {
    const cam = makeCamera(960, 540, 8.5, 12.5, 1);
    expect(tileAt(cam, 480, 270)).toEqual({ x: 8, y: 12 });
  });

  it('a click on an agent picks it, a click a tile away does not', () => {
    const cam = makeCamera(960, 540, 8, 8, 1);
    const items = [{ id: 7, x: 8, y: 8 }];
    const on = worldToScreen(cam, 8, 8);
    expect(pickAgent(items, cam, on.sx, on.sy)).toBe(7);
    const off = worldToScreen(cam, 11, 11);
    expect(pickAgent(items, cam, off.sx, off.sy)).toBeNull();
  });

  it('two agents on the same spot: the one in front wins', () => {
    const cam = makeCamera(960, 540, 0, 0, 1);
    const p = worldToScreen(cam, 0, 0);
    // Same screen column, different depth: the nearer one is drawn on top.
    const back = { id: 1, x: -0.2, y: -0.2 };
    const front = { id: 2, x: 0.2, y: 0.2 };
    expect(pickAgent([back, front], cam, p.sx, p.sy)).toBe(2);
    expect(pickAgent([front, back], cam, p.sx, p.sy)).toBe(2);
  });

  it('the pick radius is under a tile, so two neighbours never both answer', () => {
    expect(PICK_RADIUS_TILES).toBeLessThan(1);
  });
});

describe('input: camera', () => {
  it('dragging keeps the point under the pointer under the pointer', () => {
    const cam = makeCamera(960, 540, 4, 4, 1);
    const before = worldToScreen(cam, 6, 6);
    panByPixels(cam, 40, 25);
    const after = worldToScreen(cam, 6, 6);
    expect(after.sx - before.sx).toBeCloseTo(40, 6);
    expect(after.sy - before.sy).toBeCloseTo(25, 6);
  });

  it('zooming about a point keeps that point still', () => {
    const cam = makeCamera(960, 540, 4, 4, 1);
    const sx = 300;
    const sy = 200;
    const worldBefore = screenToWorld(cam, sx, sy);
    zoomAt(cam, ZOOM_STEP, sx, sy);
    const worldAfter = screenToWorld(cam, sx, sy);
    expect(cam.zoom).toBeCloseTo(ZOOM_STEP, 6);
    expect(worldAfter.x).toBeCloseTo(worldBefore.x, 6);
    expect(worldAfter.y).toBeCloseTo(worldBefore.y, 6);
  });

  it('zoom is clamped and a clamped zoom does not move the camera', () => {
    const cam = makeCamera(960, 540, 4, 4, 4);
    zoomAt(cam, 2, 100, 100);
    expect(cam.zoom).toBe(4);
    expect(cam.x).toBe(4);
    expect(cam.y).toBe(4);
  });

  it('the keys pan along the isometric axes and zoom on + and -', () => {
    expect(keyPan('ArrowUp')).toEqual({ x: -KEY_STEP, y: -KEY_STEP });
    expect(keyPan('ArrowRight')).toEqual({ x: KEY_STEP, y: -KEY_STEP });
    expect(keyPan('w')).toEqual(keyPan('ArrowUp'));
    expect(keyPan('q')).toBeNull();
    expect(keyZoom('+')).toBe(ZOOM_STEP);
    expect(keyZoom('-')).toBeCloseTo(1 / ZOOM_STEP, 12);
    expect(keyZoom('x')).toBeNull();
  });

  it('the drag threshold is a few pixels, not zero', () => {
    expect(DRAG_SLOP).toBeGreaterThan(0);
    expect(DRAG_SLOP).toBeLessThan(20);
  });
});
