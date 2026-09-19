// Sprite batcher: one interleaved VBO, depth sort, per-vertex tint, mirroring by flag.
// Pure — it only produces typed arrays and batch descriptors; the backends upload them.

import { frameUV, getFrame, type AtlasData, type AtlasFrame } from './atlas';
import { depth, worldToScreen } from './camera';
import type { Camera, SpriteInst } from './types';

/** x, y, u, v, r, g, b, a */
export const FLOATS_PER_VERTEX = 8;
export const VERTS_PER_QUAD = 4;
export const FLOATS_PER_QUAD = FLOATS_PER_VERTEX * VERTS_PER_QUAD;
export const INDICES_PER_QUAD = 6;

export interface Batch {
  /** Atlas page = texture unit binding for this run. */
  page: number;
  /** First index of the run inside the shared index buffer. */
  offset: number;
  /** Number of indices in the run. */
  count: number;
}

export interface BatchResult {
  vertices: Float32Array;
  quads: number;
  batches: Batch[];
  /** Sprites that were dropped because their frame is unknown. */
  skipped: number;
}

export interface Quad {
  page: number;
  /** Top-left in device pixels. */
  sx: number;
  sy: number;
  w: number;
  h: number;
  u0: number;
  v0: number;
  u1: number;
  v1: number;
  flip?: boolean;
  tint?: number;
  alpha?: number;
}

export function unpackTint(tint: number): [number, number, number] {
  return [((tint >> 16) & 0xff) / 255, ((tint >> 8) & 0xff) / 255, (tint & 0xff) / 255];
}

/** Writes one quad (4 vertices) at `floatOffset`; returns the next float offset. */
export function writeQuad(out: Float32Array, floatOffset: number, q: Quad): number {
  const [r, g, b] = unpackTint(q.tint ?? 0xffffff);
  const a = q.alpha ?? 1;
  // Mirroring swaps the U coordinates; the art is never duplicated.
  const ul = q.flip ? q.u1 : q.u0;
  const ur = q.flip ? q.u0 : q.u1;
  const x0 = q.sx;
  const y0 = q.sy;
  const x1 = q.sx + q.w;
  const y1 = q.sy + q.h;
  let o = floatOffset;
  // top-left, top-right, bottom-right, bottom-left
  const put = (x: number, y: number, u: number, v: number) => {
    out[o] = x;
    out[o + 1] = y;
    out[o + 2] = u;
    out[o + 3] = v;
    out[o + 4] = r;
    out[o + 5] = g;
    out[o + 6] = b;
    out[o + 7] = a;
    o += FLOATS_PER_VERTEX;
  };
  put(x0, y0, ul, q.v0);
  put(x1, y0, ur, q.v0);
  put(x1, y1, ur, q.v1);
  put(x0, y1, ul, q.v1);
  return o;
}

/** Shared static index buffer: 0,1,2, 0,2,3 per quad. */
export function buildIndices(quads: number): Uint16Array {
  const idx = new Uint16Array(quads * INDICES_PER_QUAD);
  for (let i = 0; i < quads; i++) {
    const v = i * VERTS_PER_QUAD;
    const o = i * INDICES_PER_QUAD;
    idx[o] = v;
    idx[o + 1] = v + 1;
    idx[o + 2] = v + 2;
    idx[o + 3] = v;
    idx[o + 4] = v + 2;
    idx[o + 5] = v + 3;
  }
  return idx;
}

/**
 * Sort key of a sprite: ground depth (x + y) plus elevation (z + the frame's
 * z_base). Stable: equal keys keep their input order.
 */
export function spriteDepth(s: SpriteInst, frame?: AtlasFrame): number {
  return depth(s.x, s.y, s.z + (frame ? frame.zBase / 32 : 0));
}

/** Indices of `sprites` in draw order (back to front). Stable. */
export function sortByDepth(sprites: readonly SpriteInst[], atlas?: AtlasData): number[] {
  const order = new Array<number>(sprites.length);
  const keys = new Float64Array(sprites.length);
  for (let i = 0; i < sprites.length; i++) {
    order[i] = i;
    const f = atlas?.frames.get(sprites[i].frame);
    keys[i] = spriteDepth(sprites[i], f);
  }
  order.sort((a, b) => keys[a] - keys[b] || a - b);
  return order;
}

export interface PageSize {
  w: number;
  h: number;
}

/**
 * Builds the vertex data for a sprite list, sorted by depth, with one batch per
 * run of consecutive quads on the same atlas page.
 */
export function buildSpriteBatches(
  sprites: readonly SpriteInst[],
  atlas: AtlasData,
  cam: Camera,
  pageSizes: readonly PageSize[],
  scratch?: Float32Array,
): BatchResult {
  const order = sortByDepth(sprites, atlas);
  const need = sprites.length * FLOATS_PER_QUAD;
  const out = scratch && scratch.length >= need ? scratch : new Float32Array(need);
  const batches: Batch[] = [];
  let floats = 0;
  let quads = 0;
  let skipped = 0;
  let current: Batch | null = null;

  for (const i of order) {
    const s = sprites[i];
    const f = atlas.frames.get(s.frame);
    if (!f) {
      skipped++;
      continue;
    }
    const size = pageSizes[f.page];
    if (!size) {
      skipped++;
      continue;
    }
    const uv = frameUV(f, size.w, size.h);
    const p = worldToScreen(cam, s.x, s.y, s.z);
    const w = f.w * cam.zoom;
    const h = f.h * cam.zoom;
    // The pivot is the ground contact point; mirroring mirrors the pivot too.
    const px = s.flip ? f.w - f.pivotX : f.pivotX;
    floats = writeQuad(out, floats, {
      page: f.page,
      sx: p.sx - px * cam.zoom,
      sy: p.sy - f.pivotY * cam.zoom,
      w,
      h,
      u0: uv.u0,
      v0: uv.v0,
      u1: uv.u1,
      v1: uv.v1,
      flip: s.flip,
      tint: s.tint,
      alpha: s.alpha,
    });
    if (!current || current.page !== f.page) {
      current = { page: f.page, offset: quads * INDICES_PER_QUAD, count: 0 };
      batches.push(current);
    }
    current.count += INDICES_PER_QUAD;
    quads++;
  }

  return { vertices: out, quads, batches, skipped };
}

/** Draw calls a batch result costs, plus one per visible chunk. */
export function drawCalls(batches: readonly Batch[], visibleChunks: number): number {
  return batches.length + visibleChunks;
}

/** Culls sprites whose quad falls fully outside the viewport. */
export function cullSprites(
  sprites: readonly SpriteInst[],
  atlas: AtlasData,
  cam: Camera,
  margin = 64,
): SpriteInst[] {
  const out: SpriteInst[] = [];
  for (const s of sprites) {
    const f = atlas.frames.get(s.frame);
    if (!f) continue;
    const p = worldToScreen(cam, s.x, s.y, s.z);
    const x0 = p.sx - f.pivotX * cam.zoom;
    const y0 = p.sy - f.pivotY * cam.zoom;
    if (
      x0 + f.w * cam.zoom < -margin ||
      y0 + f.h * cam.zoom < -margin ||
      x0 > cam.width + margin ||
      y0 > cam.height + margin
    ) {
      continue;
    }
    out.push(s);
  }
  return out;
}

/** Cheap reusable scratch buffer, grown in powers of two. */
export function growScratch(current: Float32Array | null, floats: number): Float32Array {
  if (current && current.length >= floats) return current;
  let size = Math.max(1024, current ? current.length : 0);
  while (size < floats) size *= 2;
  return new Float32Array(size);
}

/** Convenience used by getFrame consumers that want a throwing lookup. */
export { getFrame };
