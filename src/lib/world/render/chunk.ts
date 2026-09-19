// Chunks: the static floor and fixed decoration of a 16x16 block become ONE render
// texture, redrawn only when the chunk is dirty (decision 2 of the orchestration §4.4).
// Thousands of draw calls become one per visible chunk.

import { CHUNK_TILES, TILE_H, TILE_W, TILE_Z, type ChunkId, type ChunkTile } from './types';

export interface ChunkLayout {
  /** Texture pixels per world pixel. */
  scale: number;
  originX: number;
  originY: number;
  size: number;
}

/**
 * Layout of a chunk inside its square render texture: the diamond sits in the
 * bottom half, leaving the top half for tile elevation (walls, objects).
 */
export function chunkLayout(textureSize: number): ChunkLayout {
  const scale = textureSize / (CHUNK_TILES * TILE_W);
  return { scale, originX: textureSize / 2, originY: textureSize / 2, size: textureSize };
}

/** Projection of a tile-local position into the chunk render texture, in px. */
export function chunkTilePx(
  layout: ChunkLayout,
  lx: number,
  ly: number,
  z = 0,
): { px: number; py: number } {
  const x = (lx - ly) * (TILE_W / 2);
  const y = (lx + ly) * (TILE_H / 2) - z * TILE_Z;
  return { px: layout.originX + x * layout.scale, py: layout.originY + y * layout.scale };
}

/** Tiles sorted back to front for baking (same y + z rule as the sprites). */
export function sortTiles(tiles: readonly ChunkTile[]): ChunkTile[] {
  return Array.from(tiles).sort(
    (a, b) => a.lx + a.ly + (a.z ?? 0) - (b.lx + b.ly + (b.z ?? 0)) || a.lx - b.lx,
  );
}

export interface ChunkEntry {
  id: ChunkId;
  tiles: ChunkTile[];
  dirty: boolean;
  /** Frame counter of the last bake, for LRU eviction of chunk textures. */
  lastUsed: number;
}

/**
 * Chunk bookkeeping. Holds no GPU object: the backends keep a texture per id and
 * ask the store which ids need a bake this frame.
 */
export class ChunkStore {
  private map = new Map<ChunkId, ChunkEntry>();
  private clock = 0;

  set(id: ChunkId, tiles: ChunkTile[]): void {
    const entry = this.map.get(id);
    if (entry) {
      entry.tiles = tiles;
      entry.dirty = true;
    } else {
      this.map.set(id, { id, tiles, dirty: true, lastUsed: this.clock });
    }
  }

  invalidate(id: ChunkId): void {
    const entry = this.map.get(id);
    if (entry) entry.dirty = true;
  }

  /** Every chunk becomes dirty — used after a context loss or a tier change. */
  invalidateAll(): void {
    for (const entry of this.map.values()) entry.dirty = true;
  }

  get(id: ChunkId): ChunkEntry | undefined {
    return this.map.get(id);
  }

  has(id: ChunkId): boolean {
    return this.map.has(id);
  }

  delete(id: ChunkId): void {
    this.map.delete(id);
  }

  clear(): void {
    this.map.clear();
  }

  touch(ids: readonly ChunkId[]): void {
    this.clock++;
    for (const id of ids) {
      const e = this.map.get(id);
      if (e) e.lastUsed = this.clock;
    }
  }

  /** Dirty chunks among the visible ones, capped so a frame never stalls. */
  dirtyAmong(visible: readonly ChunkId[], max = 2): ChunkId[] {
    const out: ChunkId[] = [];
    for (const id of visible) {
      const e = this.map.get(id);
      if (e?.dirty) {
        out.push(id);
        if (out.length >= max) break;
      }
    }
    return out;
  }

  markBaked(id: ChunkId): void {
    const e = this.map.get(id);
    if (e) e.dirty = false;
  }

  /** Least recently used ids beyond `keep`, for texture eviction. */
  evictable(keep: number): ChunkId[] {
    if (this.map.size <= keep) return [];
    const all = Array.from(this.map.values()).sort((a, b) => a.lastUsed - b.lastUsed);
    return all.slice(0, this.map.size - keep).map((e) => e.id);
  }

  get size(): number {
    return this.map.size;
  }
}

/** Bytes a chunk texture costs as RGBA8. */
export function chunkTextureBytes(textureSize: number, chunks: number): number {
  return textureSize * textureSize * 4 * chunks;
}

/** How many chunk textures fit in the tier's texture budget. */
export function chunkTextureCap(textureSize: number, budgetBytes: number): number {
  if (!Number.isFinite(budgetBytes)) return 64;
  return Math.max(4, Math.floor(budgetBytes / (textureSize * textureSize * 4)));
}
