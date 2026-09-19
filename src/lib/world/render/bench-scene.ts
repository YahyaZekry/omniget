// Synthetic scene for the bench and for the hidden /world route: quads only, atlas
// generated in memory, no PNG on disk. The renderer and the bench never wait for art.

import { CHUNK_TILES, type ChunkTile, type Renderer, type Scene, type SpriteInst } from './types';
import { chunkId, makeCamera } from './camera';

export const BENCH_PAGE_SIZE = 512;
export const BENCH_TILE = 'bench/floor';
export const BENCH_WALL = 'bench/wall';
export const BENCH_AGENT_FRAMES = ['bench/agent-0', 'bench/agent-1', 'bench/agent-2', 'bench/agent-3'];

/** Atlas JSON of the synthetic sheet, in the format of static/world/atlas.schema.json. */
export function syntheticAtlasJson(): unknown {
  const frames: Record<string, unknown> = {
    [BENCH_TILE]: { page: 0, x: 0, y: 0, w: 64, h: 32, pivot: [32, 16] },
    [BENCH_WALL]: { page: 0, x: 64, y: 0, w: 64, h: 64, pivot: [32, 48], z_base: 32 },
  };
  BENCH_AGENT_FRAMES.forEach((id, i) => {
    frames[id] = { page: 0, x: 128 + i * 32, y: 0, w: 32, h: 48, pivot: [16, 46] };
  });
  return {
    version: 1,
    pages: ['bench-0.png'],
    frames,
    anims: {
      'bench/idle': {
        fps: 10,
        loop: true,
        dirs: {
          S: BENCH_AGENT_FRAMES,
          SW: BENCH_AGENT_FRAMES,
          W: BENCH_AGENT_FRAMES,
          NW: BENCH_AGENT_FRAMES,
          N: BENCH_AGENT_FRAMES,
          SE: { mirror_of: 'SW' },
          E: { mirror_of: 'W' },
          NE: { mirror_of: 'NW' },
        },
      },
    },
    tiles: {
      'floor/bench': { frame: BENCH_TILE, height: 0 },
      'wall/bench': { frame: BENCH_WALL, height: 32, occludes: true, walkable: false },
    },
  };
}

function paintPage(size: number): HTMLCanvasElement | OffscreenCanvas {
  const canvas =
    typeof OffscreenCanvas !== 'undefined'
      ? new OffscreenCanvas(size, size)
      : Object.assign(document.createElement('canvas'), { width: size, height: size });
  const ctx = canvas.getContext('2d') as
    | CanvasRenderingContext2D
    | OffscreenCanvasRenderingContext2D;
  ctx.clearRect(0, 0, size, size);
  // Floor diamond.
  ctx.fillStyle = '#3f6f4f';
  ctx.beginPath();
  ctx.moveTo(32, 0);
  ctx.lineTo(64, 16);
  ctx.lineTo(32, 32);
  ctx.lineTo(0, 16);
  ctx.closePath();
  ctx.fill();
  // Wall block.
  ctx.fillStyle = '#8a7f6a';
  ctx.fillRect(64, 0, 64, 64);
  ctx.fillStyle = '#6b6151';
  ctx.fillRect(64, 48, 64, 16);
  // Four agent frames.
  for (let i = 0; i < 4; i++) {
    const x = 128 + i * 32;
    ctx.fillStyle = ['#d86f4f', '#e08a5f', '#d86f4f', '#c05f45'][i];
    ctx.fillRect(x + 8, 8 + (i % 2), 16, 32);
    ctx.fillStyle = '#f2d7b8';
    ctx.fillRect(x + 10, 2, 12, 10);
  }
  return canvas;
}

/** Atlas pages built in memory; nothing is decoded from disk. */
export async function createSyntheticAtlas(
  size = BENCH_PAGE_SIZE,
): Promise<{ json: unknown; pages: ImageBitmap[] }> {
  const canvas = paintPage(size);
  const bitmap = await createImageBitmap(canvas as CanvasImageSource);
  return { json: syntheticAtlasJson(), pages: [bitmap] };
}

export interface BenchSceneOpts {
  sprites: number;
  chunks: number;
  texts: number;
}

/** Tiles of one synthetic chunk: a full floor plus a wall along two edges. */
export function benchChunkTiles(): ChunkTile[] {
  const tiles: ChunkTile[] = [];
  for (let ly = 0; ly < CHUNK_TILES; ly++) {
    for (let lx = 0; lx < CHUNK_TILES; lx++) {
      tiles.push({ frame: BENCH_TILE, lx, ly });
      if (lx === 0 || ly === 0) tiles.push({ frame: BENCH_WALL, lx, ly, z: 0 });
    }
  }
  return tiles;
}

/** Chunk ids of a square block of `chunks` chunks around the origin. */
export function benchChunkIds(chunks: number): string[] {
  const side = Math.max(1, Math.ceil(Math.sqrt(chunks)));
  const ids: string[] = [];
  for (let cy = 0; cy < side && ids.length < chunks; cy++) {
    for (let cx = 0; cx < side && ids.length < chunks; cx++) ids.push(chunkId(cx, cy));
  }
  return ids;
}

/**
 * Synthetic scene. Deterministic: same input, same positions, so two runs of the
 * bench are comparable.
 */
export function buildBenchScene(opts: BenchSceneOpts): Scene {
  const chunks = benchChunkIds(Math.max(1, opts.chunks));
  const side = Math.max(1, Math.ceil(Math.sqrt(chunks.length))) * CHUNK_TILES;
  const sprites: SpriteInst[] = [];
  const cols = Math.max(1, Math.ceil(Math.sqrt(Math.max(1, opts.sprites))));
  // Packed around the camera on a 1.5-tile grid so every sprite is inside the
  // 960x540 viewport at zoom 1: a scene of N sprites has to cost N sprites, not
  // whatever survives the cull.
  const spacing = 1.5;
  const centre = side / 2;
  const half = ((cols - 1) * spacing) / 2;
  for (let i = 0; i < opts.sprites; i++) {
    const col = i % cols;
    const row = Math.floor(i / cols);
    sprites.push({
      frame: BENCH_AGENT_FRAMES[i % BENCH_AGENT_FRAMES.length],
      x: centre - half + col * spacing,
      y: centre - half + row * spacing,
      z: 0,
      flip: i % 3 === 0,
      tint: 0xffffff,
      alpha: 1,
    });
  }
  const camera = makeCamera(960, 540, side / 2, side / 2, 1);
  return { camera, chunksVisible: chunks, sprites, texts: [] };
}

export function benchTextLabels(count: number): string[] {
  const out: string[] = [];
  for (let i = 0; i < count; i++) out.push(`agent-${i.toString().padStart(2, '0')}`);
  return out;
}

/**
 * Fills `scene.texts` with handles rasterised by the renderer. Separate from
 * buildBenchScene because a handle only exists once a renderer is initialised.
 */
export function attachBenchTexts(renderer: Renderer, scene: Scene, count: number): void {
  scene.texts = benchTextLabels(count).map((label, i) => {
    const sprite = scene.sprites[i % Math.max(1, scene.sprites.length)];
    return {
      handle: renderer.text(label, { size: 12, color: '#fff', outline: '#000' }),
      x: sprite ? sprite.x : i,
      y: sprite ? sprite.y : i,
      z: (sprite ? sprite.z : 0) + 1.4,
      alpha: 1,
    };
  });
}

/** Feeds every synthetic chunk to the renderer so the first frame bakes them. */
export function primeBenchChunks(renderer: Renderer, scene: Scene): void {
  const tiles = benchChunkTiles();
  for (const id of scene.chunksVisible) renderer.bakeChunk(id, tiles);
}
