// Isometric camera: pure projection maths, no GL, no DOM.

import { CHUNK_TILES, TILE_H, TILE_W, TILE_Z, type Camera, type ChunkId } from './types';

const HALF_W = TILE_W / 2;
const HALF_H = TILE_H / 2;

export function makeCamera(width: number, height: number, x = 0, y = 0, zoom = 1): Camera {
  return { x, y, zoom, width, height };
}

/** Projection of a world point into unzoomed, uncentred screen pixels. */
export function project(x: number, y: number, z = 0): { px: number; py: number } {
  return { px: (x - y) * HALF_W, py: (x + y) * HALF_H - z * TILE_Z };
}

/** World point to device pixels inside the canvas. */
export function worldToScreen(
  cam: Camera,
  x: number,
  y: number,
  z = 0,
): { sx: number; sy: number } {
  const p = project(x, y, z);
  const c = project(cam.x, cam.y, 0);
  return {
    sx: (p.px - c.px) * cam.zoom + cam.width / 2,
    sy: (p.py - c.py) * cam.zoom + cam.height / 2,
  };
}

/** Device pixels back to the world plane z = 0. */
export function screenToWorld(cam: Camera, sx: number, sy: number): { x: number; y: number } {
  const c = project(cam.x, cam.y, 0);
  const px = (sx - cam.width / 2) / cam.zoom + c.px;
  const py = (sy - cam.height / 2) / cam.zoom + c.py;
  const a = px / HALF_W;
  const b = py / HALF_H;
  return { x: (a + b) / 2, y: (b - a) / 2 };
}

/**
 * Ground depth of a world point: the screen-space depth axis of the isometric
 * projection (x + y) plus the elevation z. This is the "sort by y + z" rule of
 * docs/agents/f6-renderer.md §4 expressed in world coordinates, where the depth
 * axis of the diamond grid is x + y rather than y alone.
 */
export function depth(x: number, y: number, z = 0): number {
  return x + y + z;
}

/** Axis-aligned world bounds visible by the camera, at z = 0, in tile units. */
export function visibleBounds(
  cam: Camera,
  marginTiles = 2,
): { minX: number; minY: number; maxX: number; maxY: number } {
  const corners = [
    screenToWorld(cam, 0, 0),
    screenToWorld(cam, cam.width, 0),
    screenToWorld(cam, 0, cam.height),
    screenToWorld(cam, cam.width, cam.height),
  ];
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const c of corners) {
    if (c.x < minX) minX = c.x;
    if (c.x > maxX) maxX = c.x;
    if (c.y < minY) minY = c.y;
    if (c.y > maxY) maxY = c.y;
  }
  return {
    minX: minX - marginTiles,
    minY: minY - marginTiles,
    maxX: maxX + marginTiles,
    maxY: maxY + marginTiles,
  };
}

export function chunkId(cx: number, cy: number): ChunkId {
  return `${cx},${cy}`;
}

export function parseChunkId(id: ChunkId): { cx: number; cy: number } {
  const i = id.indexOf(',');
  return { cx: Number(id.slice(0, i)), cy: Number(id.slice(i + 1)) };
}

/** Chunks touched by the camera, in draw order (back to front). */
export function visibleChunks(cam: Camera, marginTiles = 2): ChunkId[] {
  const b = visibleBounds(cam, marginTiles);
  const cx0 = Math.floor(b.minX / CHUNK_TILES);
  const cx1 = Math.floor(b.maxX / CHUNK_TILES);
  const cy0 = Math.floor(b.minY / CHUNK_TILES);
  const cy1 = Math.floor(b.maxY / CHUNK_TILES);
  const out: ChunkId[] = [];
  for (let cy = cy0; cy <= cy1; cy++) {
    for (let cx = cx0; cx <= cx1; cx++) out.push(chunkId(cx, cy));
  }
  out.sort((a, b2) => {
    const pa = parseChunkId(a);
    const pb = parseChunkId(b2);
    return pa.cx + pa.cy - (pb.cx + pb.cy) || pa.cx - pb.cx;
  });
  return out;
}

/** Screen rect (device px) covered by a chunk's diamond, at z = 0. */
export function chunkScreenRect(
  cam: Camera,
  id: ChunkId,
): { x: number; y: number; w: number; h: number } {
  const { cx, cy } = parseChunkId(id);
  const ox = cx * CHUNK_TILES;
  const oy = cy * CHUNK_TILES;
  const top = worldToScreen(cam, ox, oy);
  const right = worldToScreen(cam, ox + CHUNK_TILES, oy);
  const left = worldToScreen(cam, ox, oy + CHUNK_TILES);
  return {
    x: left.sx,
    y: top.sy,
    w: right.sx - left.sx,
    h: worldToScreen(cam, ox + CHUNK_TILES, oy + CHUNK_TILES).sy - top.sy,
  };
}

export function clampZoom(zoom: number): number {
  return Math.min(4, Math.max(0.25, zoom));
}
