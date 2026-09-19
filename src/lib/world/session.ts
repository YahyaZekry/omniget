// The route's half of the bridge contract (docs/agents/f7-world-bridge.md §4).
//
// Commands: world_exists, world_create, world_open, world_close, world_input,
// world_set_visible, world_tier_get, world_tier_set, world_calibration_save,
// world_delete. The snapshot comes back from `world_open` and every diff after
// it arrives on a `tauri::ipc::Channel<Vec<u8>>`; the only event is
// `world://sleep-state`.
//
// Everything that touches Tauri is in this file, behind an interface, so the
// state machine above it can be driven by `createFakeSession` in a test with
// real blobs and no app.

import { decodeBlob, type Blob as WorldBlob } from './codec';

/** The inputs the route can send. Mirrors `omniget_world::Input`. */
export type WorldInput =
  | { type: 'move'; ent: number; to: [number, number] }
  | { type: 'interact'; ent: number; object: number }
  | { type: 'place_object'; object: number; kind: string; tile: [number, number]; dir: number; slot?: string | null }
  | { type: 'remove_object'; object: number }
  | { type: 'say'; ent: number; text: string }
  | { type: 'sleep'; ent: number }
  | { type: 'idle'; ent: number };

export type SleepStateName = 'active' | 'dozing' | 'hibernating' | 'never';

export interface OpenResult {
  /** The snapshot the bridge answered `world_open` with. */
  blob: WorldBlob;
}

export interface WorldSession {
  exists(): Promise<boolean>;
  create(opts?: { seed?: number; house?: string }): Promise<void>;
  /** Opens the world and starts the diff stream. Resolves with the first snapshot. */
  open(onBlob: (blob: WorldBlob) => void, onError: (code: string) => void): Promise<OpenResult>;
  close(): Promise<void>;
  send(input: WorldInput): Promise<void>;
  setVisible(visible: boolean): Promise<void>;
  /** Asks the bridge for a fresh snapshot after a gap or a map mismatch. */
  resync(): Promise<void>;
  delete(): Promise<void>;
  onSleepState(cb: (state: SleepStateName) => void): () => void;
  dispose(): void;
}

/**
 * Bytes out of a channel message. Tauri may hand a channel payload over as a
 * raw ArrayBuffer or as a JSON array of numbers depending on how the Rust side
 * sends it, and the route must not care which.
 */
export function toBytes(message: unknown): Uint8Array {
  if (message instanceof Uint8Array) return message;
  if (message instanceof ArrayBuffer) return new Uint8Array(message);
  if (ArrayBuffer.isView(message)) {
    const v = message as ArrayBufferView;
    return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  }
  if (Array.isArray(message)) return Uint8Array.from(message as number[]);
  const data = (message as { data?: unknown } | null)?.data;
  if (data) return toBytes(data);
  throw new TypeError('ERR_WORLD_BAD_CHANNEL_PAYLOAD');
}

/**
 * A visit to someone else's open house. Same interface as home: the first
 * snapshot is the answer of `house_join`, the diffs come through the channel,
 * and the only input that means anything to the host is "walk there".
 */
export function createVisitSession(visit: { code: string; server?: string | null }): WorldSession {
  const home = createTauriSession();
  async function core() {
    return await import('@tauri-apps/api/core');
  }
  return {
    ...home,
    async exists() {
      return true;
    },
    async create() {},
    async open(onBlob, onError) {
      const { invoke, Channel } = await core();
      const ch = new Channel<unknown>();
      ch.onmessage = (message: unknown) => {
        try {
          onBlob(decodeBlob(toBytes(message)));
        } catch (e) {
          onError(e instanceof Error ? e.message : String(e));
        }
      };
      const first = await invoke('house_join', { code: visit.code, server: visit.server ?? null, channel: ch });
      return { blob: decodeBlob(toBytes(first)) };
    },
    async close() {
      const { invoke } = await core();
      await invoke('house_leave');
    },
    async send(input) {
      const { invoke } = await core();
      await invoke('house_input', { input });
    },
    async setVisible() {},
    async resync() {
      // The host answers an interest frame with a whole snapshot.
      const { invoke } = await core();
      await invoke('house_input', { input: { type: 'interest' } });
    },
    async delete() {},
    onSleepState() {
      return () => {};
    },
  };
}

export function createTauriSession(): WorldSession {
  let unlisten: (() => void) | null = null;
  let channel: { onmessage: (m: unknown) => void } | null = null;

  async function core() {
    return await import('@tauri-apps/api/core');
  }

  return {
    async exists() {
      const { invoke } = await core();
      return (await invoke('world_exists')) as boolean;
    },
    async create(opts) {
      const { invoke } = await core();
      await invoke('world_create', { seed: opts?.seed ?? null, house: opts?.house ?? 'casa-v1' });
    },
    async open(onBlob, onError) {
      const { invoke, Channel } = await core();
      const ch = new Channel<unknown>();
      ch.onmessage = (message: unknown) => {
        try {
          onBlob(decodeBlob(toBytes(message)));
        } catch (e) {
          onError(e instanceof Error ? e.message : String(e));
        }
      };
      channel = ch as unknown as { onmessage: (m: unknown) => void };
      const first = await invoke('world_open', { channel: ch });
      return { blob: decodeBlob(toBytes(first)) };
    },
    async close() {
      const { invoke } = await core();
      await invoke('world_close');
    },
    async send(input) {
      const { invoke } = await core();
      await invoke('world_input', { input });
    },
    async setVisible(visible) {
      const { invoke } = await core();
      await invoke('world_set_visible', { visible });
    },
    async resync() {
      // Asks for a whole snapshot on the channel that is already open. Waking
      // the world up (what this used to do) sends no snapshot, so a replica
      // that lost one blob stayed lost for good.
      const { invoke } = await core();
      await invoke('world_resync');
    },
    async delete() {
      const { invoke } = await core();
      await invoke('world_delete');
    },
    onSleepState(cb) {
      let dead = false;
      void (async () => {
        const { listen } = await import('@tauri-apps/api/event');
        const stop = await listen<SleepStateName | { state: SleepStateName }>(
          'world://sleep-state',
          (ev) => {
            const p = ev.payload as SleepStateName | { state: SleepStateName };
            cb(typeof p === 'string' ? p : p.state);
          },
        );
        if (dead) stop();
        else unlisten = stop;
      })();
      return () => {
        dead = true;
        unlisten?.();
        unlisten = null;
      };
    },
    dispose() {
      unlisten?.();
      unlisten = null;
      if (channel) channel.onmessage = () => {};
      channel = null;
    },
  };
}

export interface FakeSessionControls extends WorldSession {
  /** Hands the next fixture blob to the route, as the channel would. */
  advance(): boolean;
  /** Every input the route sent, in order. */
  readonly sent: WorldInput[];
  readonly opened: boolean;
  readonly visible: boolean;
  /** Feeds arbitrary bytes, to exercise the error path. */
  push(bytes: Uint8Array): void;
}

/**
 * A session driven by fixture blobs instead of by Rust. The route cannot tell
 * the difference, which is the point: the whole state machine is testable
 * without an app, a GPU or a tick thread.
 */
export function createFakeSession(blobs: Uint8Array[]): FakeSessionControls {
  let index = 0;
  let onBlob: ((b: WorldBlob) => void) | null = null;
  let onError: ((code: string) => void) | null = null;
  let sleepCb: ((s: SleepStateName) => void) | null = null;
  const sent: WorldInput[] = [];
  let opened = false;
  let visible = true;
  let exists = blobs.length > 0;

  const session: FakeSessionControls = {
    async exists() {
      return exists;
    },
    async create() {
      exists = true;
    },
    async open(next, err) {
      onBlob = next;
      onError = err;
      opened = true;
      if (blobs.length === 0) throw new Error('ERR_WORLD_NO_FIXTURE');
      index = 1;
      return { blob: decodeBlob(blobs[0]) };
    },
    async close() {
      opened = false;
    },
    async send(input) {
      sent.push(input);
    },
    async setVisible(v) {
      visible = v;
      sleepCb?.(v ? 'active' : 'dozing');
    },
    async resync() {
      index = 0;
      session.advance();
    },
    async delete() {
      exists = false;
      opened = false;
    },
    onSleepState(cb) {
      sleepCb = cb;
      return () => {
        sleepCb = null;
      };
    },
    dispose() {
      onBlob = null;
      onError = null;
      sleepCb = null;
    },
    advance() {
      if (index >= blobs.length) return false;
      session.push(blobs[index++]);
      return true;
    },
    push(bytes: Uint8Array) {
      try {
        onBlob?.(decodeBlob(bytes));
      } catch (e) {
        onError?.(e instanceof Error ? e.message : String(e));
      }
    },
    get sent() {
      return sent;
    },
    get opened() {
      return opened;
    },
    get visible() {
      return visible;
    },
  };
  return session;
}

/** True inside the Tauri webview; outside it the route says so instead of throwing. */
export function inTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
