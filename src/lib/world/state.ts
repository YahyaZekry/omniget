// The replica of the world inside the webview.
//
// The authority is the Rust `World` on the tick thread; this keeps what the
// screen needs and nothing else: where each agent is, which clip it plays,
// how much energy it has, what it just said, and the objects of the house.
// It applies diffs in place, because at 10 Hz a rebuild would be the single
// most expensive thing the route does.
//
// Two refusals matter and both are named, because a replica that quietly
// drifts from the authority is worse than one that asks for a new snapshot:
//   - ERR_WORLD_MAP_MISMATCH: the blob belongs to another house;
//   - ERR_WORLD_DIFF_GAP: a blob was dropped, so the state is a guess now.
// Either one sets `needsResync`, and the session answers it with a snapshot.

import {
  FIELD_ACT,
  FIELD_ANIM,
  FIELD_DESPAWNED,
  FIELD_DIR,
  FIELD_ENERGY,
  FIELD_POS,
  type Activity,
  type Diff,
  type ObjectState,
  type Snapshot,
  type WorldEvent,
} from './codec';
import { ArrivalClock, retarget, SNAP_AFTER_MS, still, toTiles, type Track } from './interp';

export const STATE_ERR = {
  MAP_MISMATCH: 'ERR_WORLD_MAP_MISMATCH',
  DIFF_GAP: 'ERR_WORLD_DIFF_GAP',
} as const;

/** How long a speech balloon stays on screen after the event that carried it. */
export const SAY_MS = 4500;
/** Events kept for the HUD log; older ones fall off the back. */
export const EVENT_LOG_MAX = 64;

export interface AgentView {
  id: number;
  name: string;
  dir: number;
  anim: number;
  energy: number;
  activity: Activity;
  track: Track;
  /** `nowMs` at which the current clip started, for the sprite clock. */
  animStartedMs: number;
  /** What the agent is saying, while it lasts. */
  saying: string;
  sayingUntilMs: number;
}

export interface ApplyResult {
  ok: boolean;
  code?: string;
}

export interface StateStats {
  snapshots: number;
  diffs: number;
  /** Milliseconds the last `applyDiff` took. */
  lastApplyMs: number;
  maxApplyMs: number;
  resyncs: number;
}

/** A `Map` in insertion order would draw agents in arrival order; ids are stable. */
function byId(a: AgentView, b: AgentView): number {
  return a.id - b.id;
}

export class WorldState {
  tick = 0;
  seed = 0n;
  mapHash = 0n;
  sleep: Snapshot['sleep'] = 'active';
  /** Keyed by EntId. */
  readonly agents = new Map<number, AgentView>();
  /** Keyed by ObjectId. */
  readonly objects = new Map<number, ObjectState>();
  /** Newest last, at most EVENT_LOG_MAX. */
  readonly events: WorldEvent[] = [];
  readonly clock = new ArrivalClock();
  needsResync = false;
  ready = false;
  readonly stats: StateStats = {
    snapshots: 0,
    diffs: 0,
    lastApplyMs: 0,
    maxApplyMs: 0,
    resyncs: 0,
  };
  /** Bumped whenever the object layer changes, so chunks are re-baked once. */
  objectsVersion = 0;

  private sorted: AgentView[] = [];
  private sortedDirty = true;

  /** Agents in ascending id order; the array is reused between frames. */
  list(): AgentView[] {
    if (this.sortedDirty) {
      this.sorted = [...this.agents.values()].sort(byId);
      this.sortedDirty = false;
    }
    return this.sorted;
  }

  applySnapshot(s: Snapshot, nowMs: number): ApplyResult {
    this.tick = s.tick;
    this.seed = s.seed;
    this.mapHash = s.mapHash;
    this.sleep = s.sleep;
    this.agents.clear();
    this.objects.clear();
    for (const a of s.agents) {
      this.agents.set(a.id, {
        id: a.id,
        name: a.name,
        dir: a.dir,
        anim: a.anim,
        energy: a.energy,
        activity: a.activity,
        track: still(toTiles(a.x), toTiles(a.y), toTiles(a.z), nowMs),
        animStartedMs: nowMs,
        saying: '',
        sayingUntilMs: 0,
      });
    }
    for (const o of s.objects) this.objects.set(o.id, o);
    this.objectsVersion++;
    this.sortedDirty = true;
    this.events.length = 0;
    this.clock.reset();
    this.clock.mark(nowMs);
    this.needsResync = false;
    this.ready = true;
    this.stats.snapshots++;
    return { ok: true };
  }

  applyDiff(d: Diff, nowMs: number): ApplyResult {
    const t0 = performance.now();
    if (!this.ready) return this.refuse(STATE_ERR.DIFF_GAP);
    if (d.mapHash !== this.mapHash) return this.refuse(STATE_ERR.MAP_MISMATCH);
    if (d.from !== this.tick) return this.refuse(STATE_ERR.DIFF_GAP);

    // A blob that arrives after a long silence (the world was dozing or
    // hibernating) snaps; a normal one is stretched over the measured mean of
    // the last few gaps, which is smoother than the raw one.
    const gap = this.clock.mark(nowMs);
    const trackMs = gap >= SNAP_AFTER_MS ? gap : this.clock.trackMs;

    for (const delta of d.ents) {
      if (delta.mask & FIELD_DESPAWNED) {
        // The codec guarantees this bit carries no other.
        if (this.agents.delete(delta.id)) this.sortedDirty = true;
        continue;
      }
      let a = this.agents.get(delta.id);
      if (!a) {
        a = {
          id: delta.id,
          name: delta.name ?? '',
          dir: delta.dir,
          anim: delta.anim,
          energy: delta.energy,
          activity: delta.activity,
          track: still(toTiles(delta.x), toTiles(delta.y), toTiles(delta.z), nowMs),
          animStartedMs: nowMs,
          saying: '',
          sayingUntilMs: 0,
        };
        this.agents.set(delta.id, a);
        this.sortedDirty = true;
        continue;
      }
      if (delta.name !== null) a.name = delta.name;
      if (delta.mask & FIELD_POS) {
        retarget(a.track, toTiles(delta.x), toTiles(delta.y), toTiles(delta.z), nowMs, trackMs);
      }
      if (delta.mask & FIELD_DIR) a.dir = delta.dir;
      if (delta.mask & FIELD_ANIM && a.anim !== delta.anim) {
        a.anim = delta.anim;
        a.animStartedMs = nowMs;
      }
      if (delta.mask & FIELD_ENERGY) a.energy = delta.energy;
      if (delta.mask & FIELD_ACT) a.activity = delta.activity;
    }

    if (d.objects.length > 0) {
      for (const od of d.objects) {
        if (od.op === 'removed') this.objects.delete(od.id);
        else this.objects.set(od.id, od.object);
      }
      this.objectsVersion++;
    }

    for (const e of d.events) this.pushEvent(e, nowMs);

    this.tick = d.to;
    this.sleep = d.sleep;
    this.stats.diffs++;
    const ms = performance.now() - t0;
    this.stats.lastApplyMs = ms;
    if (ms > this.stats.maxApplyMs) this.stats.maxApplyMs = ms;
    return { ok: true };
  }

  private refuse(code: string): ApplyResult {
    this.needsResync = true;
    this.stats.resyncs++;
    return { ok: false, code };
  }

  private pushEvent(e: WorldEvent, nowMs: number): void {
    if (e.type === 'said') {
      const a = this.agents.get(e.ent);
      if (a) {
        a.saying = e.text;
        a.sayingUntilMs = nowMs + SAY_MS;
      }
    }
    this.events.push(e);
    if (this.events.length > EVENT_LOG_MAX) this.events.shift();
  }

  /** Agents with a live balloon, for the tier cap on balloons. */
  speaking(nowMs: number): AgentView[] {
    const out: AgentView[] = [];
    for (const a of this.list()) if (a.sayingUntilMs > nowMs) out.push(a);
    return out;
  }

  /** Lowest energy on screen. */
  lowestEnergy(): number {
    let min = 255;
    for (const a of this.agents.values()) if (a.energy < min) min = a.energy;
    return min;
  }

  /**
   * Highest energy on screen. This is the one that decides whether the house
   * goes dark: the plan (§9.1) darkens it when *every* account is spent, so
   * one tired agent among rested ones must not turn the lights off.
   */
  highestEnergy(): number {
    let max = 0;
    for (const a of this.agents.values()) if (a.energy > max) max = a.energy;
    return max;
  }

  clear(): void {
    this.agents.clear();
    this.objects.clear();
    this.events.length = 0;
    this.sortedDirty = true;
    this.ready = false;
    this.needsResync = false;
    this.tick = 0;
    this.mapHash = 0n;
    this.clock.reset();
  }
}
