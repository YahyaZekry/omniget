// WebGL1 fallback: same renderer, asked for version 1. No VAO, no instancing, one
// texture bound per batch — `gl2.ts` already writes the version-1 shaders and skips
// the vertex-array object when version === 1.

import { createGlRenderer } from './gl2';
import type { Renderer } from './types';

export function createGl1Renderer(): Renderer {
  return createGlRenderer(1);
}

export default createGl1Renderer;
