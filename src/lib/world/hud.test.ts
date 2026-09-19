import { describe, expect, it } from 'vitest';
import { makeCamera } from '$lib/world/render/camera';
import type { TextHandle, TextStyle } from '$lib/world/render/types';
import {
  BAR_BG_FRAME,
  balloonBudget,
  buildHud,
  capAgents,
  energyFrame,
  energyLevel,
  energyTint,
  fadeOut,
  gameClock,
  gameClockLabel,
  untilLabel,
  type AgentFrame,
} from './hud';
import { WorldState } from './state';
import { still } from './interp';
import type { AgentView } from './state';

function agent(id: number, over: Partial<AgentView> = {}): AgentView {
  return {
    id,
    name: `agent-${id}`,
    dir: 0,
    anim: 0,
    energy: 255,
    activity: { tag: 0, name: 'idle', arg: 0 },
    track: still(id, id, 0, 0),
    animStartedMs: 0,
    saying: '',
    sayingUntilMs: 0,
    ...over,
  };
}

function frames(agents: AgentView[]): AgentFrame[] {
  return agents.map((a) => ({ agent: a, x: a.track.toX, y: a.track.toY, z: 0 }));
}

function textFactory() {
  const asked: string[] = [];
  let id = 0;
  const text = (str: string, _style?: TextStyle): TextHandle => {
    asked.push(str);
    id++;
    return { id, w: 40, h: 14 };
  };
  return { text, asked };
}

function hudOpts(over: Partial<Parameters<typeof buildHud>[2]> = {}) {
  const { text, asked } = textFactory();
  return {
    opts: {
      tier: 2 as const,
      nowMs: 1000,
      camera: makeCamera(960, 540, 0, 0, 1),
      hovered: null,
      selected: null,
      dark: false,
      text,
      ...over,
    },
    asked,
  };
}

describe('hud: energy as the bar', () => {
  it('maps the 0..255 byte onto nine steps', () => {
    expect(energyLevel(0)).toBe(0);
    expect(energyLevel(255)).toBe(8);
    expect(energyLevel(128)).toBe(4);
    expect(energyLevel(Number.NaN)).toBe(0);
    expect(energyFrame(255)).toBe('ui/bar/8');
  });

  it('the colour follows the crate thresholds, not a guess', () => {
    expect(energyTint(255)).toBe(energyTint(64));
    expect(energyTint(63)).not.toBe(energyTint(64));
    expect(energyTint(15)).not.toBe(energyTint(63));
  });

  it('every agent gets a track and a fill', () => {
    const state = new WorldState();
    const list = [agent(1), agent(2, { energy: 10 })];
    const { opts } = hudOpts();
    const built = buildHud(state, frames(list), opts);
    const bars = built.sprites.filter((s) => s.frame === BAR_BG_FRAME);
    expect(bars).toHaveLength(2);
    expect(built.sprites.filter((s) => s.frame.startsWith('ui/bar/'))).toHaveLength(4);
  });
});

describe('hud: what each tier is allowed to draw', () => {
  it('tier 0 shows a name only under the pointer', () => {
    const state = new WorldState();
    const list = [agent(1), agent(2)];
    const { opts, asked } = hudOpts({ tier: 0, hovered: 2 });
    buildHud(state, frames(list), opts);
    expect(asked).toEqual(['agent-2']);
  });

  it('tier 1 shows every name and exactly one balloon', () => {
    const state = new WorldState();
    const list = [
      agent(1, { saying: 'oi', sayingUntilMs: 5000 }),
      agent(2, { saying: 'olá', sayingUntilMs: 5000 }),
    ];
    const { opts, asked } = hudOpts({ tier: 1 });
    buildHud(state, frames(list), opts);
    expect(asked.filter((s) => s === 'oi' || s === 'olá')).toHaveLength(1);
    expect(asked.filter((s) => s.startsWith('agent-'))).toHaveLength(2);
  });

  it('tier 2 shows every balloon', () => {
    const state = new WorldState();
    const list = [
      agent(1, { saying: 'oi', sayingUntilMs: 5000 }),
      agent(2, { saying: 'olá', sayingUntilMs: 5000 }),
    ];
    const { opts, asked } = hudOpts({ tier: 2 });
    buildHud(state, frames(list), opts);
    expect(asked.filter((s) => s === 'oi' || s === 'olá')).toHaveLength(2);
  });

  it('a tier-1 frame stays inside the ten-line budget with eight agents', () => {
    const state = new WorldState();
    const list = Array.from({ length: 8 }, (_, i) =>
      agent(i + 1, { saying: 'oi', sayingUntilMs: 5000 }),
    );
    const { opts } = hudOpts({ tier: 1, clockLine: '07:30' });
    const built = buildHud(state, frames(list), opts);
    expect(built.textCount).toBeLessThanOrEqual(10);
  });

  it('the balloon budget per tier is the tier table, not a number typed twice', () => {
    expect(balloonBudget(0)).toBe(0);
    expect(balloonBudget(1)).toBe(1);
    expect(balloonBudget(3)).toBe(Number.POSITIVE_INFINITY);
  });

  it('over the tier cap only the nearest agents are drawn', () => {
    const cam = makeCamera(960, 540, 0, 0, 1);
    const list = Array.from({ length: 30 }, (_, i) => agent(i + 1));
    const kept = capAgents(frames(list), 1, cam);
    expect(kept).toHaveLength(8);
    expect(kept.map((f) => f.agent.id)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });
});

describe('hud: the clocks', () => {
  it('eight ticks are a game minute', () => {
    expect(gameClock(0)).toEqual({ hour: 0, minute: 0 });
    expect(gameClock(8)).toEqual({ hour: 0, minute: 1 });
    expect(gameClock(8 * 60)).toEqual({ hour: 1, minute: 0 });
    expect(gameClockLabel(8 * (7 * 60 + 30))).toBe('07:30');
    // A full day (11520 ticks) wraps back to midnight.
    expect(gameClockLabel(11_520)).toBe('00:00');
  });

  it('the time until the window flips reads like the quota meter', () => {
    expect(untilLabel(0)).toBe('');
    expect(untilLabel(-5)).toBe('');
    expect(untilLabel(45 * 60_000)).toBe('45 min');
    expect(untilLabel(3 * 3_600_000 + 12 * 60_000)).toBe('3 h 12 min');
  });

  it('the balloon fades out instead of blinking', () => {
    expect(fadeOut(2000)).toBe(1);
    expect(fadeOut(250)).toBeCloseTo(0.5, 6);
    expect(fadeOut(0)).toBe(0);
  });

  it('the clock line is anchored to the canvas, not to the world', () => {
    const state = new WorldState();
    const { opts } = hudOpts({ clockLine: '07:30' });
    const a = buildHud(state, [], opts);
    const moved = makeCamera(960, 540, 40, 40, 1);
    const b = buildHud(state, [], { ...opts, camera: moved });
    expect(a.texts).toHaveLength(1);
    // The camera moved by 40 tiles, so the world position of the line moved
    // with it: that is what keeps it at the top of the screen.
    expect(b.texts[0].x - a.texts[0].x).toBeCloseTo(40, 6);
  });
});

describe('hud: the house going dark', () => {
  it('tints every layer when the accounts are spent', () => {
    const state = new WorldState();
    const list = [agent(1, { energy: 8 })];
    const { opts } = hudOpts({ dark: true });
    const built = buildHud(state, frames(list), opts);
    for (const s of built.sprites) expect(s.tint).toBe(0x5a6070);
  });
});
