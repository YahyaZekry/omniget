// The HUD, inside the canvas.
//
// Decision 6 of the orchestration §4.4: names, balloons and bars over a canvas
// as DOM elements are the most reliable way to stall a weak machine. So every
// label here is a rasterised line from the renderer's text atlas and every bar
// is a sprite, and the whole HUD costs one extra draw call.
//
// What the tier table (§1.2) allows:
//   tier 0  'hover'   only the hovered name, no balloon
//   tier 1  'names+1' every name plus one balloon
//   tier 2+ 'all'     every name and every balloon
//
// Pure module: it builds lists, it draws nothing and it holds no state.

import { budgetForTier } from '$lib/world/render/tier';
import { TILE_Z, type Camera, type SpriteInst, type TextHandle, type TextInst, type TextStyle, type Tier } from '$lib/world/render/types';
import { screenToWorld } from '$lib/world/render/camera';
import type { AgentView, WorldState } from './state';

/** Elevation, in tiles, at which each HUD layer floats over the agent's feet. */
export const BAR_Z = 2.15;
export const NAME_Z = 2.5;
export const BALLOON_Z = 3.1;

/** Nine discrete fills; a variable-width bar would need a frame per pixel. */
export const BAR_STEPS = 8;
export const BAR_FRAME_PREFIX = 'ui/bar/';
export const BAR_BG_FRAME = 'ui/bar/bg';

/** Tint applied to everything when every account is spent (plan §9.1). */
export const DARK_TINT = 0x5a6070;
export const LIT_TINT = 0xffffff;

/** Energy thresholds of the crate: tired at 64, spent at 16, over 255. */
export const ENERGY_TIRED = 64;
export const ENERGY_SPENT = 16;

export const NAME_STYLE: TextStyle = { size: 12, color: '#f2f4f8', outline: '#11131a' };
export const BALLOON_STYLE: TextStyle = { size: 12, color: '#11131a', outline: '#f6f7fb', maxWidth: 220 };
export const CLOCK_STYLE: TextStyle = { size: 12, color: '#e8eaf0', outline: '#11131a' };

/** An agent already placed by the interpolator, in tile units. */
export interface AgentFrame {
  agent: AgentView;
  x: number;
  y: number;
  z: number;
}

export interface HudOpts {
  tier: Tier;
  nowMs: number;
  camera: Camera;
  /** Agent under the pointer, or null. */
  hovered: number | null;
  /** Agent the user clicked, whose name and bar stay pinned. */
  selected: number | null;
  /** True when every account is under 10%: the house goes dark until the window flips. */
  dark: boolean;
  /**
   * Device pixels per CSS pixel. Text is rasterised in device pixels, so
   * without this a 12 px name is 6 px tall on a retina screen.
   */
  scale?: number;
  /** In-game clock line, already formatted and translated by the caller. */
  clockLine?: string;
  /** Rasterises one line; the renderer keeps an LRU of 256 of them. */
  text(str: string, style?: TextStyle): TextHandle;
}

export interface HudBuild {
  sprites: SpriteInst[];
  texts: TextInst[];
  /** For the budget assertion: how many lines this frame asked for. */
  textCount: number;
}

const scaledStyles = new Map<string, TextStyle>();

/** `style` at `scale`, the same object every time so the text atlas can key on it. */
export function scaledStyle(style: TextStyle, scale: number | undefined): TextStyle {
  const k = Math.round(Math.min(3, Math.max(1, scale ?? 1)) * 4) / 4;
  if (k === 1) return style;
  const key = `${k}|${style.size}|${style.color}|${style.outline}|${style.maxWidth ?? 0}`;
  let out = scaledStyles.get(key);
  if (!out) {
    out = {
      ...style,
      size: Math.round((style.size ?? 12) * k),
      maxWidth: style.maxWidth === undefined ? undefined : Math.round(style.maxWidth * k),
    };
    scaledStyles.set(key, out);
  }
  return out;
}

/** 0..BAR_STEPS from the raw 0..255 energy byte. */
export function energyLevel(energy: number): number {
  if (!Number.isFinite(energy)) return 0;
  const clamped = Math.min(255, Math.max(0, energy));
  return Math.round((clamped / 255) * BAR_STEPS);
}

export function energyFrame(energy: number): string {
  return `${BAR_FRAME_PREFIX}${energyLevel(energy)}`;
}

/** Green while rested, amber while tired, red while spent — plus the bar length. */
export function energyTint(energy: number): number {
  if (energy < ENERGY_SPENT) return 0xff6b5c;
  if (energy < ENERGY_TIRED) return 0xffb648;
  return 0x66d38a;
}

/** Ticks of the crate to a wall clock of the game day (8 ticks = 1 game minute). */
export const TICKS_PER_GAME_MINUTE = 8;
export const MINUTES_PER_DAY = 1440;

export function gameClock(tick: number): { hour: number; minute: number } {
  const minutes = Math.floor(tick / TICKS_PER_GAME_MINUTE) % MINUTES_PER_DAY;
  return { hour: Math.floor(minutes / 60), minute: minutes % 60 };
}

export function gameClockLabel(tick: number): string {
  const { hour, minute } = gameClock(tick);
  return `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`;
}

/** "3 h 12 min" until the quota window flips; empty when there is no flip known. */
export function untilLabel(msLeft: number): string {
  if (!Number.isFinite(msLeft) || msLeft <= 0) return '';
  const mins = Math.round(msLeft / 60_000);
  const h = Math.floor(mins / 60);
  return h > 0 ? `${h} h ${mins % 60} min` : `${mins} min`;
}

/** A point fixed to the canvas rather than to the world, for the clock line. */
export function screenAnchor(cam: Camera, sx: number, sy: number): { x: number; y: number } {
  return screenToWorld(cam, sx, sy);
}

/** Balloons this tier is allowed to show at once. */
export function balloonBudget(tier: Tier): number {
  const text = budgetForTier(tier).text;
  if (text === 'hover') return 0;
  return text === 'names+1' ? 1 : Number.POSITIVE_INFINITY;
}

/** True when this agent's name is drawn: hovered and selected always are. */
export function showsName(tier: Tier, agent: AgentView, opts: HudOpts): boolean {
  if (opts.hovered === agent.id || opts.selected === agent.id) return true;
  return budgetForTier(tier).text !== 'hover';
}

/**
 * Everything the HUD adds to the scene this frame. Reads state, allocates two
 * arrays and returns; the caller concatenates them onto the scene it already
 * built for the agents themselves.
 */
export function buildHud(state: WorldState, frames: AgentFrame[], opts: HudOpts): HudBuild {
  const sprites: SpriteInst[] = [];
  const texts: TextInst[] = [];
  const tint = opts.dark ? DARK_TINT : LIT_TINT;
  let balloonsLeft = balloonBudget(opts.tier);

  for (const f of frames) {
    const a = f.agent;
    // The bar is two sprites: the empty track and the fill, so the fill never
    // has to be scaled (the batcher draws frames at their atlas size).
    sprites.push({ frame: BAR_BG_FRAME, x: f.x, y: f.y, z: f.z + BAR_Z, tint, alpha: 0.75 });
    sprites.push({
      frame: energyFrame(a.energy),
      x: f.x,
      y: f.y,
      z: f.z + BAR_Z,
      tint: opts.dark ? DARK_TINT : energyTint(a.energy),
    });

    if (showsName(opts.tier, a, opts) && a.name) {
      texts.push({
        handle: opts.text(a.name, scaledStyle(NAME_STYLE, opts.scale)),
        x: f.x,
        y: f.y,
        z: f.z + NAME_Z,
        tint,
      });
    }

    if (balloonsLeft > 0 && a.sayingUntilMs > opts.nowMs && a.saying) {
      balloonsLeft--;
      texts.push({
        handle: opts.text(a.saying, scaledStyle(BALLOON_STYLE, opts.scale)),
        x: f.x,
        y: f.y,
        z: f.z + balloonZ(opts),
        alpha: fadeOut(a.sayingUntilMs - opts.nowMs),
      });
    }
  }

  if (opts.clockLine) {
    const at = screenAnchor(opts.camera, opts.camera.width / 2, 28);
    texts.push({ handle: opts.text(opts.clockLine, scaledStyle(CLOCK_STYLE, opts.scale)), x: at.x, y: at.y, z: 0, tint });
  }

  return { sprites, texts, textCount: texts.length };
}

/**
 * Elevation of a balloon: one line of text above the name, whatever the zoom.
 * Elevations are in tiles and text is in pixels, so a fixed gap that clears the
 * name at zoom 1 lands on top of it once the camera pulls back.
 */
export function balloonZ(opts: Pick<HudOpts, 'camera' | 'scale'>): number {
  const linePx = (NAME_STYLE.size ?? 12) * Math.min(3, Math.max(1, opts.scale ?? 1)) * 1.5;
  const zoom = opts.camera.zoom > 0 ? opts.camera.zoom : 1;
  return Math.max(BALLOON_Z, NAME_Z + linePx / (TILE_Z * zoom));
}

/** The last half second of a balloon fades instead of blinking out. */
export function fadeOut(msLeft: number): number {
  if (msLeft >= 500) return 1;
  return Math.max(0, msLeft / 500);
}

/**
 * The agents this tier animates: the ones nearest the camera. Over the cap the
 * rest are simply not drawn, which is the declared degradation of §1.2 rather
 * than a frame rate that quietly falls apart.
 */
export function capAgents(frames: AgentFrame[], tier: Tier, cam: Camera): AgentFrame[] {
  const cap = budgetForTier(tier).agents;
  if (frames.length <= cap) return frames;
  const near = [...frames].sort((a, b) => dist2(a, cam) - dist2(b, cam));
  near.length = cap;
  return near;
}

function dist2(f: AgentFrame, cam: Camera): number {
  const dx = f.x - cam.x;
  const dy = f.y - cam.y;
  return dx * dx + dy * dy;
}
