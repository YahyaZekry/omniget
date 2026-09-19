// Decoder for the binary world wire format, ported line by line from
// `src-tauri/omniget-world/src/snapshot/{binary,mod,diff}.rs`, whose doc
// comments are the normative specification.
//
// The same bytes leave the tick thread through a `tauri::ipc::Channel` ten
// times a second and, one day, come off a socket from a room server. That is
// why this file is a hand-written reader over a `Uint8Array` and not JSON:
// a walking agent costs nine bytes here.
//
// Two rules the Rust reader enforces and this one must enforce too, or the
// two halves can disagree in silence:
//   1. a blob with bytes left over is an error (ERR_WORLD_TRAILING);
//   2. an unknown tag, op, mask or animation id is an error, never a shrug.
//
// Numbers: `u64` fields that are identities rather than quantities (seed,
// rng_state, map_hash) come back as `bigint`, because FNV-64 routinely
// exceeds 2^53 and a double would round it into a different house. Counters
// the UI does arithmetic on (tick, ids, indices) come back as `number` and a
// value past `Number.MAX_SAFE_INTEGER` is refused instead of rounded.

/** ASCII "OGW1". */
export const MAGIC = Object.freeze([0x4f, 0x47, 0x57, 0x31]);
export const FORMAT_VERSION = 1;
export const KIND_SNAPSHOT = 1;
export const KIND_DIFF = 2;
export const HEADER_LEN = 6;
export const MAX_STRING_BYTES = 4096;
export const MAX_STRINGS = 4096;
export const MAX_ITEMS = 1 << 20;
/** Animation ids 0..7; the decoder refuses anything else (`ANIM_COUNT` in Rust). */
export const ANIM_COUNT = 8;
/** Sub-units of a tile in the fixed-point coordinates (`Fixed::ONE`). */
export const FIXED_ONE = 256;

export const FIELD_POS = 1 << 0;
export const FIELD_DIR = 1 << 1;
export const FIELD_ANIM = 1 << 2;
export const FIELD_ENERGY = 1 << 3;
export const FIELD_ACT = 1 << 4;
export const FIELD_SPAWNED = 1 << 5;
export const FIELD_DESPAWNED = 1 << 6;
/** What a spawn carries: a complete record, so a late client needs no extra message. */
export const FIELD_FULL =
  FIELD_POS | FIELD_DIR | FIELD_ANIM | FIELD_ENERGY | FIELD_ACT | FIELD_SPAWNED;

/** Stable codes, same spelling as `WorldError::code()` in Rust. */
export const CODEC_ERR = {
  BAD_MAGIC: 'ERR_WORLD_BAD_MAGIC',
  BAD_VERSION: 'ERR_WORLD_BAD_VERSION',
  BAD_KIND: 'ERR_WORLD_BAD_KIND',
  TRUNCATED: 'ERR_WORLD_TRUNCATED',
  VARINT: 'ERR_WORLD_VARINT',
  BAD_STRING: 'ERR_WORLD_BAD_STRING',
  BAD_TAG: 'ERR_WORLD_BAD_TAG',
  TRAILING: 'ERR_WORLD_TRAILING',
  TOO_LONG: 'ERR_WORLD_TOO_LONG',
} as const;

export type CodecErrCode = (typeof CODEC_ERR)[keyof typeof CODEC_ERR];

export class CodecError extends Error {
  readonly code: CodecErrCode;
  constructor(code: CodecErrCode, message?: string) {
    super(message ? `${code}: ${message}` : code);
    this.code = code;
    this.name = 'CodecError';
  }
}

export type SleepStateName = 'active' | 'dozing' | 'hibernating';
const SLEEP_STATES: SleepStateName[] = ['active', 'dozing', 'hibernating'];

/** Activity tags 0..7 (`Activity::from_parts`). The arg is an ObjectId or an EntId. */
export const ACTIVITY_NAMES = [
  'idle',
  'walking',
  'sitting',
  'sleeping',
  'working',
  'talking',
  'waving',
  'yawning',
] as const;
export type ActivityName = (typeof ACTIVITY_NAMES)[number];

export interface Activity {
  tag: number;
  name: ActivityName;
  /** ObjectId for sitting/working, EntId for waving, 0 otherwise. */
  arg: number;
}

export interface AgentState {
  id: number;
  name: string;
  /** Fixed-point raw coordinates: 256 sub-units to the tile. */
  x: number;
  y: number;
  z: number;
  dir: number;
  anim: number;
  energy: number;
  activity: Activity;
}

export interface ObjectState {
  id: number;
  kind: string;
  tx: number;
  ty: number;
  dir: number;
  walkable: boolean;
  /** Atlas pixels of height, for depth sorting and occlusion. */
  height: number;
  footprint: [number, number];
  slot: string | null;
}

export interface Snapshot {
  tick: number;
  seed: bigint;
  rngState: bigint;
  mapHash: bigint;
  sleep: SleepStateName;
  agents: AgentState[];
  objects: ObjectState[];
}

export interface EntDelta {
  id: number;
  mask: number;
  name: string | null;
  x: number;
  y: number;
  z: number;
  dir: number;
  anim: number;
  energy: number;
  activity: Activity;
}

export type ObjDelta =
  | { op: 'removed'; id: number }
  | { op: 'placed'; id: number; object: ObjectState };

export type WorldEvent =
  | { type: 'arrived'; ent: number; tx: number; ty: number }
  | { type: 'started_anim'; ent: number; anim: number; dir: number }
  | { type: 'said'; ent: number; text: string }
  | { type: 'interacted'; ent: number; object: number }
  | { type: 'slept'; ent: number }
  | { type: 'woke'; ent: number }
  | { type: 'yawned'; ent: number }
  | { type: 'spawned'; ent: number; tx: number; ty: number }
  | { type: 'despawned'; ent: number }
  | { type: 'object_placed'; object: number; tx: number; ty: number }
  | { type: 'object_removed'; object: number }
  | { type: 'caught_up'; ticks: number }
  | { type: 'rejected'; ent: number; code: string };

export interface Diff {
  from: number;
  to: number;
  mapHash: bigint;
  rngState: bigint;
  sleep: SleepStateName;
  ents: EntDelta[];
  objects: ObjDelta[];
  events: WorldEvent[];
}

export type Blob =
  | { kind: 'snapshot'; snapshot: Snapshot }
  | { kind: 'diff'; diff: Diff };

const UTF8 = typeof TextDecoder === 'undefined' ? null : new TextDecoder('utf-8', { fatal: true });

/** Bounds-checked byte source. Every read can fail; none can read past the end. */
class Reader {
  private pos = 0;
  constructor(private readonly buf: Uint8Array) {}

  header(): number {
    if (this.buf.length < HEADER_LEN) {
      throw new CodecError(CODEC_ERR.TRUNCATED, `header needs ${HEADER_LEN} bytes`);
    }
    for (let i = 0; i < 4; i++) {
      if (this.buf[i] !== MAGIC[i]) throw new CodecError(CODEC_ERR.BAD_MAGIC);
    }
    const kind = this.buf[4];
    const version = this.buf[5];
    if (version !== FORMAT_VERSION) {
      throw new CodecError(CODEC_ERR.BAD_VERSION, `blob format ${version}, this build reads 1`);
    }
    if (kind !== KIND_SNAPSHOT && kind !== KIND_DIFF) {
      throw new CodecError(CODEC_ERR.BAD_KIND, String(kind));
    }
    this.pos = HEADER_LEN;
    return kind;
  }

  u8(): number {
    if (this.pos >= this.buf.length) {
      throw new CodecError(CODEC_ERR.TRUNCATED, `1 byte at ${this.pos}`);
    }
    return this.buf[this.pos++];
  }

  /**
   * LEB128, accumulated by multiplication so the result stays exact up to
   * 2^53. Ten bytes without a terminator is a malformed varint, and a value
   * past the safe integer range is refused rather than rounded.
   */
  varint(): number {
    const start = this.pos;
    let result = 0;
    let scale = 1;
    for (let i = 0; i < 10; i++) {
      const b = this.u8();
      result += (b & 0x7f) * scale;
      if ((b & 0x80) === 0) {
        if (!Number.isSafeInteger(result)) {
          throw new CodecError(CODEC_ERR.TOO_LONG, `varint at ${start} exceeds 2^53`);
        }
        return result;
      }
      scale *= 128;
    }
    throw new CodecError(CODEC_ERR.VARINT, `unterminated at ${start}`);
  }

  /** The same varint as a bigint, for the u64 identities. */
  varint64(): bigint {
    const start = this.pos;
    let result = 0n;
    let shift = 0n;
    for (let i = 0; i < 10; i++) {
      const b = this.u8();
      result |= BigInt(b & 0x7f) << shift;
      if ((b & 0x80) === 0) return BigInt.asUintN(64, result);
      shift += 7n;
    }
    throw new CodecError(CODEC_ERR.VARINT, `unterminated at ${start}`);
  }

  /** Zigzag varint: `(n << 1) ^ (n >> 31)` undone, exactly as the Rust reader does. */
  zigzag(): number {
    const u = this.varint() >>> 0;
    return (u >>> 1) ^ -(u & 1);
  }

  /** A length that is refused before anything is allocated for it. */
  count(max: number): number {
    const n = this.varint();
    if (n > max || n > this.buf.length - this.pos + 1) {
      throw new CodecError(CODEC_ERR.TOO_LONG, `count ${n} over ${max}`);
    }
    return n;
  }

  bytes(): Uint8Array {
    const n = this.varint();
    if (this.pos + n > this.buf.length) {
      throw new CodecError(CODEC_ERR.TRUNCATED, `${n} bytes at ${this.pos}`);
    }
    const out = this.buf.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  get remaining(): number {
    return this.buf.length - this.pos;
  }

  /** Bytes left over mean the writer and the reader disagree: that is a bug, not a tail. */
  finish(): void {
    if (this.remaining > 0) {
      throw new CodecError(CODEC_ERR.TRAILING, `${this.remaining} bytes`);
    }
  }
}

function readStringTable(r: Reader): string[] {
  const n = r.count(MAX_STRINGS);
  const items: string[] = new Array(n);
  for (let i = 0; i < n; i++) {
    const raw = r.bytes();
    if (raw.length > MAX_STRING_BYTES) {
      throw new CodecError(CODEC_ERR.TOO_LONG, `string ${i} is ${raw.length} bytes`);
    }
    if (!UTF8) throw new CodecError(CODEC_ERR.BAD_STRING, 'no TextDecoder in this runtime');
    try {
      items[i] = UTF8.decode(raw);
    } catch {
      throw new CodecError(CODEC_ERR.BAD_STRING, `string ${i} is not UTF-8`);
    }
  }
  return items;
}

function str(table: string[], idx: number): string {
  const s = table[idx];
  if (s === undefined) throw new CodecError(CODEC_ERR.BAD_STRING, `index ${idx}`);
  return s;
}

function sleepState(tag: number): SleepStateName {
  const s = SLEEP_STATES[tag];
  if (s === undefined) throw new CodecError(CODEC_ERR.BAD_TAG, `sleep state ${tag}`);
  return s;
}

function activity(tag: number, arg: number): Activity {
  const name = ACTIVITY_NAMES[tag];
  if (name === undefined) throw new CodecError(CODEC_ERR.BAD_TAG, `activity ${tag}`);
  return { tag, name, arg };
}

function checkAnim(anim: number): number {
  if (anim >= ANIM_COUNT) throw new CodecError(CODEC_ERR.BAD_TAG, `anim ${anim}`);
  return anim;
}

function readObject(r: Reader, table: string[], id: number): ObjectState {
  const kind = str(table, r.varint());
  const tx = r.zigzag();
  const ty = r.zigzag();
  const dir = r.u8();
  const walkable = r.u8() !== 0;
  const height = r.varint() & 0xffff;
  const footprint: [number, number] = [r.u8(), r.u8()];
  const slotIdx = r.varint();
  return {
    id,
    kind,
    tx,
    ty,
    dir,
    walkable,
    height,
    footprint,
    slot: slotIdx === 0 ? null : str(table, slotIdx - 1),
  };
}

export function decodeSnapshot(bytes: Uint8Array): Snapshot {
  const r = new Reader(bytes);
  const kind = r.header();
  if (kind !== KIND_SNAPSHOT) throw new CodecError(CODEC_ERR.BAD_KIND, String(kind));
  const snapshot = readSnapshotBody(r);
  r.finish();
  return snapshot;
}

function readSnapshotBody(r: Reader): Snapshot {
  const tick = r.varint();
  const seed = r.varint64();
  const rngState = r.varint64();
  const mapHash = r.varint64();
  const sleep = sleepState(r.u8());
  const table = readStringTable(r);

  const nAgents = r.count(MAX_ITEMS);
  const agents: AgentState[] = new Array(nAgents);
  for (let i = 0; i < nAgents; i++) {
    const id = r.varint();
    const name = str(table, r.varint());
    const x = r.zigzag();
    const y = r.zigzag();
    const z = r.zigzag();
    const dir = r.u8();
    const anim = r.u8();
    const energy = r.u8();
    const actTag = r.u8();
    const actArg = r.varint();
    checkAnim(anim);
    agents[i] = { id, name, x, y, z, dir, anim, energy, activity: activity(actTag, actArg) };
  }

  const nObjects = r.count(MAX_ITEMS);
  const objects: ObjectState[] = new Array(nObjects);
  for (let i = 0; i < nObjects; i++) {
    objects[i] = readObject(r, table, r.varint());
  }

  return { tick, seed, rngState, mapHash, sleep, agents, objects };
}

export function decodeDiff(bytes: Uint8Array): Diff {
  const r = new Reader(bytes);
  const kind = r.header();
  if (kind !== KIND_DIFF) throw new CodecError(CODEC_ERR.BAD_KIND, String(kind));
  const diff = readDiffBody(r);
  r.finish();
  return diff;
}

const IDLE: Activity = Object.freeze({ tag: 0, name: 'idle' as const, arg: 0 });

function readDiffBody(r: Reader): Diff {
  const from = r.varint();
  const to = r.varint();
  const mapHash = r.varint64();
  const rngState = r.varint64();
  const sleep = sleepState(r.u8());
  const table = readStringTable(r);

  const nEnts = r.count(MAX_ITEMS);
  const ents: EntDelta[] = new Array(nEnts);
  for (let i = 0; i < nEnts; i++) {
    const id = r.varint();
    const mask = r.u8();
    if (mask & FIELD_DESPAWNED && mask !== FIELD_DESPAWNED) {
      throw new CodecError(CODEC_ERR.BAD_TAG, `entity delta mask ${mask}`);
    }
    const d: EntDelta = {
      id,
      mask,
      name: null,
      x: 0,
      y: 0,
      z: 0,
      dir: 0,
      anim: 0,
      energy: 0,
      activity: IDLE,
    };
    if (mask & FIELD_POS) {
      d.x = r.zigzag();
      d.y = r.zigzag();
      d.z = r.zigzag();
    }
    if (mask & FIELD_DIR) d.dir = r.u8();
    if (mask & FIELD_ANIM) d.anim = checkAnim(r.u8());
    if (mask & FIELD_ENERGY) d.energy = r.u8();
    if (mask & FIELD_ACT) {
      const tag = r.u8();
      d.activity = activity(tag, r.varint());
    }
    if (mask & FIELD_SPAWNED) d.name = str(table, r.varint());
    ents[i] = d;
  }

  const nObjects = r.count(MAX_ITEMS);
  const objects: ObjDelta[] = new Array(nObjects);
  for (let i = 0; i < nObjects; i++) {
    const id = r.varint();
    const op = r.u8();
    if (op === 0) objects[i] = { op: 'removed', id };
    else if (op === 1) objects[i] = { op: 'placed', id, object: readObject(r, table, id) };
    else throw new CodecError(CODEC_ERR.BAD_TAG, `object delta op ${op}`);
  }

  const nEvents = r.count(MAX_ITEMS);
  const events: WorldEvent[] = new Array(nEvents);
  for (let i = 0; i < nEvents; i++) {
    events[i] = readEvent(r, table);
  }

  return { from, to, mapHash, rngState, sleep, ents, objects, events };
}

function readEvent(r: Reader, table: string[]): WorldEvent {
  const tag = r.u8();
  switch (tag) {
    case 0:
      return { type: 'arrived', ent: r.varint(), tx: r.zigzag(), ty: r.zigzag() };
    case 1:
      return { type: 'started_anim', ent: r.varint(), anim: checkAnim(r.u8()), dir: r.u8() };
    case 2:
      return { type: 'said', ent: r.varint(), text: str(table, r.varint()) };
    case 3:
      return { type: 'interacted', ent: r.varint(), object: r.varint() };
    case 4:
      return { type: 'slept', ent: r.varint() };
    case 5:
      return { type: 'woke', ent: r.varint() };
    case 6:
      return { type: 'yawned', ent: r.varint() };
    case 7:
      return { type: 'spawned', ent: r.varint(), tx: r.zigzag(), ty: r.zigzag() };
    case 8:
      return { type: 'despawned', ent: r.varint() };
    case 9:
      return { type: 'object_placed', object: r.varint(), tx: r.zigzag(), ty: r.zigzag() };
    case 10:
      return { type: 'object_removed', object: r.varint() };
    case 11:
      return { type: 'caught_up', ticks: r.varint() };
    case 12:
      return { type: 'rejected', ent: r.varint(), code: str(table, r.varint()) };
    default:
      throw new CodecError(CODEC_ERR.BAD_TAG, `event ${tag}`);
  }
}

/** Reads whichever of the two blob kinds arrived on the channel. */
export function decodeBlob(bytes: Uint8Array): Blob {
  if (bytes.length >= HEADER_LEN && bytes[4] === KIND_DIFF) {
    return { kind: 'diff', diff: decodeDiff(bytes) };
  }
  return { kind: 'snapshot', snapshot: decodeSnapshot(bytes) };
}

/** Sub-unit coordinate to tiles, the only conversion the renderer needs. */
export function toTiles(fixed: number): number {
  return fixed / FIXED_ONE;
}

/** The error code of anything this module threw, for the UI to show. */
export function codecErrorCode(e: unknown): string {
  return e instanceof CodecError ? e.code : e instanceof Error ? e.message : String(e);
}
