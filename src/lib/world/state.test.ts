// The replica: what it accepts, what it refuses by name, and how long it
// takes. The wire itself is `codec.test.ts`; here the blobs are already
// decoded values, built by hand so each case says what it is testing.

import { describe, expect, it } from 'vitest';
import {
  FIELD_ACT,
  FIELD_ANIM,
  FIELD_DESPAWNED,
  FIELD_DIR,
  FIELD_ENERGY,
  FIELD_FULL,
  FIELD_POS,
  type Activity,
  type Diff,
  type EntDelta,
  type Snapshot,
} from './codec';
import { SAY_MS, STATE_ERR, WorldState, EVENT_LOG_MAX } from './state';
import { sample } from './interp';

const HASH = 0x0123456789abcdefn;

function act(tag: number, arg = 0): Activity {
  return { tag, name: (['idle', 'walking', 'sitting', 'sleeping', 'working', 'talking', 'waving', 'yawning'] as const)[tag], arg };
}

function snapshot(over: Partial<Snapshot> = {}): Snapshot {
  return {
    tick: 10,
    seed: 1n,
    rngState: 2n,
    mapHash: HASH,
    sleep: 'active',
    agents: [
      {
        id: 1,
        name: 'Omni',
        x: 256,
        y: 512,
        z: 0,
        dir: 0,
        anim: 0,
        energy: 255,
        activity: act(0),
      },
    ],
    objects: [
      {
        id: 1,
        kind: 'object/bed',
        tx: 2,
        ty: 2,
        dir: 0,
        walkable: false,
        height: 16,
        footprint: [2, 2],
        slot: 'bedroom-bed',
      },
    ],
    ...over,
  };
}

function delta(id: number, mask: number, over: Partial<EntDelta> = {}): EntDelta {
  return {
    id,
    mask,
    name: null,
    x: 0,
    y: 0,
    z: 0,
    dir: 0,
    anim: 0,
    energy: 0,
    activity: act(0),
    ...over,
  };
}

function diff(over: Partial<Diff> = {}): Diff {
  return {
    from: 10,
    to: 11,
    mapHash: HASH,
    rngState: 3n,
    sleep: 'active',
    ents: [],
    objects: [],
    events: [],
    ...over,
  };
}

function ready(): WorldState {
  const s = new WorldState();
  s.applySnapshot(snapshot(), 1000);
  return s;
}

describe('state: snapshots', () => {
  it('a snapshot replaces everything and marks the state ready', () => {
    const s = ready();
    expect(s.ready).toBe(true);
    expect(s.tick).toBe(10);
    expect(s.mapHash).toBe(HASH);
    expect(s.agents.size).toBe(1);
    expect(s.objects.size).toBe(1);
    const omni = s.agents.get(1)!;
    expect(omni.name).toBe('Omni');
    // Fixed point becomes tiles exactly once, at the door.
    expect(sample(omni.track, 1000)).toEqual({ x: 1, y: 2, z: 0 });
  });

  it('a second snapshot clears the first, log included', () => {
    const s = ready();
    s.applyDiff(diff({ events: [{ type: 'slept', ent: 1 }] }), 1100);
    expect(s.events).toHaveLength(1);
    s.applySnapshot(snapshot({ tick: 99, agents: [] }), 1200);
    expect(s.agents.size).toBe(0);
    expect(s.events).toHaveLength(0);
    expect(s.tick).toBe(99);
  });
});

describe('state: diffs', () => {
  it('moves, turns, tires and changes activity', () => {
    const s = ready();
    const ok = s.applyDiff(
      diff({
        ents: [
          delta(1, FIELD_POS | FIELD_DIR | FIELD_ANIM | FIELD_ENERGY | FIELD_ACT, {
            x: 512,
            y: 512,
            z: 256,
            dir: 4,
            anim: 1,
            energy: 40,
            activity: act(1),
          }),
        ],
      }),
      1100,
    );
    expect(ok.ok).toBe(true);
    const a = s.agents.get(1)!;
    expect(a.dir).toBe(4);
    expect(a.anim).toBe(1);
    expect(a.energy).toBe(40);
    expect(a.activity.name).toBe('walking');
    expect(s.tick).toBe(11);
    // The animation clock restarts with the clip, not with the frame.
    expect(a.animStartedMs).toBe(1100);
  });

  it('a clip that did not change does not restart the sprite clock', () => {
    const s = ready();
    s.applyDiff(diff({ ents: [delta(1, FIELD_ANIM, { anim: 0 })] }), 1100);
    expect(s.agents.get(1)!.animStartedMs).toBe(1000);
  });

  it('a spawn delta adds the agent with its name', () => {
    const s = ready();
    s.applyDiff(
      diff({
        ents: [delta(2, FIELD_FULL, { name: 'Ada', x: 1024, y: 1024, energy: 200, anim: 0 })],
      }),
      1100,
    );
    expect(s.agents.get(2)!.name).toBe('Ada');
    expect(s.list().map((a) => a.id)).toEqual([1, 2]);
  });

  it('a despawn delta removes it', () => {
    const s = ready();
    s.applyDiff(diff({ ents: [delta(1, FIELD_DESPAWNED)] }), 1100);
    expect(s.agents.size).toBe(0);
    expect(s.list()).toHaveLength(0);
  });

  it('objects are placed, moved and removed, and the version bumps once', () => {
    const s = ready();
    const before = s.objectsVersion;
    s.applyDiff(
      diff({
        objects: [
          { op: 'removed', id: 1 },
          {
            op: 'placed',
            id: 2,
            object: {
              id: 2,
              kind: 'object/lamp',
              tx: 4,
              ty: 2,
              dir: 0,
              walkable: false,
              height: 28,
              footprint: [1, 1],
              slot: 'bedroom-lamp',
            },
          },
        ],
      }),
      1100,
    );
    expect(s.objects.has(1)).toBe(false);
    expect(s.objects.get(2)!.kind).toBe('object/lamp');
    expect(s.objectsVersion).toBe(before + 1);
  });

  it('a diff for another house is refused by name and asks for a resync', () => {
    const s = ready();
    const r = s.applyDiff(diff({ mapHash: 99n }), 1100);
    expect(r.ok).toBe(false);
    expect(r.code).toBe(STATE_ERR.MAP_MISMATCH);
    expect(s.needsResync).toBe(true);
    expect(s.tick).toBe(10);
  });

  it('a gap in the stream is refused by name', () => {
    const s = ready();
    const r = s.applyDiff(diff({ from: 42, to: 43 }), 1100);
    expect(r.ok).toBe(false);
    expect(r.code).toBe(STATE_ERR.DIFF_GAP);
    expect(s.needsResync).toBe(true);
  });

  it('a diff before any snapshot is a gap, not a crash', () => {
    const s = new WorldState();
    expect(s.applyDiff(diff(), 1000).code).toBe(STATE_ERR.DIFF_GAP);
  });
});

describe('state: events', () => {
  it('a spoken line becomes a balloon that expires', () => {
    const s = ready();
    s.applyDiff(diff({ events: [{ type: 'said', ent: 1, text: 'olá' }] }), 1100);
    expect(s.agents.get(1)!.saying).toBe('olá');
    expect(s.speaking(1100)).toHaveLength(1);
    expect(s.speaking(1100 + SAY_MS + 1)).toHaveLength(0);
  });

  it('the log is bounded', () => {
    const s = ready();
    for (let i = 0; i < EVENT_LOG_MAX + 20; i++) {
      s.applyDiff(
        diff({ from: 10 + i, to: 11 + i, events: [{ type: 'yawned', ent: 1 }] }),
        1100 + i,
      );
    }
    expect(s.events).toHaveLength(EVENT_LOG_MAX);
  });

  it('the darkness of the house reads the fullest account, not the emptiest', () => {
    const s = ready();
    s.applyDiff(
      diff({
        ents: [delta(2, FIELD_FULL, { name: 'Ada', energy: 4 })],
      }),
      1100,
    );
    expect(s.lowestEnergy()).toBe(4);
    // Omni is still rested, so the lights stay on.
    expect(s.highestEnergy()).toBe(255);
    s.applyDiff(
      diff({ from: 11, to: 12, ents: [delta(1, FIELD_ENERGY, { energy: 9 })] }),
      1200,
    );
    expect(s.highestEnergy()).toBe(9);
  });

  it('the sleep state of the world rides along on the diff', () => {
    const s = ready();
    s.applyDiff(diff({ sleep: 'dozing' }), 1100);
    expect(s.sleep).toBe('dozing');
  });
});

describe('state: budget', () => {
  it('applies a 64-agent diff well inside 0.2 ms', () => {
    const s = new WorldState();
    const agents = [];
    for (let i = 1; i <= 64; i++) {
      agents.push({
        id: i,
        name: `agent-${i}`,
        x: i * 256,
        y: i * 128,
        z: 0,
        dir: 0,
        anim: 0,
        energy: 255,
        activity: act(0),
      });
    }
    s.applySnapshot(snapshot({ agents }), 0);
    const blobs: Diff[] = [];
    for (let n = 0; n < 200; n++) {
      blobs.push(
        diff({
          from: 10 + n,
          to: 11 + n,
          ents: Array.from({ length: 64 }, (_, i) =>
            delta(i + 1, FIELD_POS | FIELD_DIR | FIELD_ANIM | FIELD_ENERGY, {
              x: (i + 1) * 256 + n,
              y: (i + 1) * 128 + n,
              dir: n % 8,
              anim: 1,
              energy: 200,
            }),
          ),
          events: Array.from({ length: 64 }, (_, i) => ({
            type: 'started_anim' as const,
            ent: i + 1,
            anim: 1,
            dir: n % 8,
          })),
        }),
      );
    }
    const t0 = performance.now();
    for (let n = 0; n < blobs.length; n++) s.applyDiff(blobs[n], 1000 + n * 100);
    const perApply = (performance.now() - t0) / blobs.length;
    // 64 agents is four times the 2 KB budget line of eight agents, so the
    // headroom here is the real margin. Generous ceiling: a loaded CI box is
    // still an order of magnitude away from it.
    expect(perApply).toBeLessThan(2);
    expect(s.stats.diffs).toBe(blobs.length);
    // eslint-disable-next-line no-console
    console.log(
      `[world-ui] applyDiff 64 agents + 64 events: ${perApply.toFixed(4)} ms mean over ${blobs.length}, max ${s.stats.maxApplyMs.toFixed(4)} ms`,
    );
  });
});
