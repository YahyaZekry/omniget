// The codec against blobs produced by the Rust crate.
//
// The base64 strings below are the output of `omniget-world` itself: a tiny
// throwaway binary (kept out of the repo, in the agent's scratchpad) linked
// against the crate, which encoded `Snapshot`/`Diff` values and printed the
// bytes. Two of them come from a real `World` loaded with
// `tests/fixtures/house-v1.min.json`, three agents spawned and five ticks run.
// Decoding them here is what proves the two implementations agree; the local
// writer further down exists only to build the malformed blobs that no honest
// encoder would ever produce.

import { describe, expect, it } from 'vitest';
import {
  ACTIVITY_NAMES,
  CODEC_ERR,
  CodecError,
  FIELD_ACT,
  FIELD_ANIM,
  FIELD_DESPAWNED,
  FIELD_DIR,
  FIELD_ENERGY,
  FIELD_FULL,
  FIELD_POS,
  FIELD_SPAWNED,
  HEADER_LEN,
  KIND_DIFF,
  KIND_SNAPSHOT,
  decodeBlob,
  decodeDiff,
  decodeSnapshot,
  toTiles,
} from './codec';

/** Blobs encoded by the Rust crate (see the header comment). */
const FIXTURES = {
  /** Two agents, one object in a slot, sleep = dozing. 98 bytes. */
  snapshot_sample:
    'T0dXMQEB0glj7/229Q3vm6/N+KzRkQEBBARPbW5pA0FkYRBvYmplY3Qvd29ya2JlbmNoC3N0dWR5LWJlbmNoAgEAgAr/AQADAcgBAAIBAgSABAAECgQHAQcCCAcCABQCAQQ=',
  /** Every field mask, both object ops and all thirteen event tags. 153 bytes. */
  diff_sample:
    'T0dXMQIBCgvvm6/N+KzRkQEHAAQDQWRhC29iamVjdC9sYW1wC29sw6EsIG11bmRvFUVSUl9XT1JMRF9VTlJFQUNIQUJMRQQBB9cEgAgABgECP4AEgAQAAAD/AAAAAxgMAgkEQAIDAAUBAQMSBAEcAQEADQABBg0BAQIFAgICAwIFBAMFAwYDBwICAggECQUDEgoDC8AWDAED',
  /** 64 agents moving at once: the shape of the 2 KB budget line. 924 bytes. */
  diff_64:
    'T0dXMQIBZGXvm6/N+KzRkQEBAABAAQ+SArUBAAEByAIPpATrAgACAcgDD7YGoQQAAwHIBA/ICNcFAAQByAUP2gqNBwAFAcgGD+wMwwgABgHIBw/+DvkJAAcByAgPkBGvCwAAAcgJD6IT5QwAAQHICg+0FZsOAAIByAsPxhfRDwADAcgMD9gZhxEABAHIDQ/qG70SAAUByA4P/B3zEwAGAcgPD44gqRUABwHIEA+gIt8WAAAByBEPsiSVGAABAcgSD8QmyxkAAgHIEw/WKIEbAAMByBQP6Cq3HAAEAcgVD/os7R0ABQHIFg+ML6MfAAYByBcPnjHZIAAHAcgYD7AzjyIAAAHIGQ/CNcUjAAEByBoP1Df7JAACAcgbD+Y5sSYAAwHIHA/4O+cnAAQByB0Pij6dKQAFAcgeD5xA0yoABgHIHw+uQoksAAcByCAPwES/LQAAAcghD9JG9S4AAQHIIg/kSKswAAIByCMP9krhMQADAcgkD4hNlzMABAHIJQ+aT800AAUByCYPrFGDNgAGAcgnD75TuTcABwHIKA/QVe84AAAByCkP4lelOgABAcgqD/RZ2zsAAgHIKw+GXJE9AAMByCwPmF7HPgAEAcgtD6pg/T8ABQHILg+8YrNBAAYByC8PzmTpQgAHAcgwD+Bmn0QAAAHIMQ/yaNVFAAEByDIPhGuLRwACAcgzD5ZtwUgAAwHINA+ob/dJAAQByDUPunGtSwAFAcg2D8xz40wABgHINw/edZlOAAcByDgP8HfPTwAAAcg5D4J6hVEAAQHIOg+UfLtSAAIByDsPpn7xUwADAcg8D7iAAadVAAQByD0PyoIB3VYABQHIPg/chAGTWAAGAcg/D+6GAclZAAcByEAPgIkB/1oAAAHIAEABAQEBAQIBAgEDAQMBBAEEAQUBBQEGAQYBBwEHAQgBAAEJAQEBCgECAQsBAwEMAQQBDQEFAQ4BBgEPAQcBEAEAAREBAQESAQIBEwEDARQBBAEVAQUBFgEGARcBBwEYAQABGQEBARoBAgEbAQMBHAEEAR0BBQEeAQYBHwEHASABAAEhAQEBIgECASMBAwEkAQQBJQEFASYBBgEnAQcBKAEAASkBAQEqAQIBKwEDASwBBAEtAQUBLgEGAS8BBwEwAQABMQEBATIBAgEzAQMBNAEEATUBBQE2AQYBNwEHATgBAAE5AQEBOgECATsBAwE8AQQBPQEFAT4BBgE/AQcBQAEA',
  /** A real World: three agents in house-v1.min, nine objects. 386 bytes. */
  snapshot_real:
    'T0dXMQEBAQcH8MCI86m15MLmAQAVBE9tbmkDQWRhA1JleApvYmplY3QvYmVkC2JlZHJvb20tYmVkC29iamVjdC9sYW1wDGJlZHJvb20tbGFtcApvYmplY3QvcnVnCmxpdmluZy1ydWcJb2JqZWN0L3R2CWxpdmluZy10dgxvYmplY3QvcGxhbnQMbGl2aW5nLXBsYW50EG9iamVjdC93b3JrYmVuY2gLc3R1ZHktYmVuY2gMb2JqZWN0L2NoYWlyC3N0dWR5LWNoYWlyEG9iamVjdC9ib29rc2hlbGYLc3R1ZHktc2hlbGYMb2JqZWN0L3RhYmxlC3N0dWR5LXRhYmxlAwEAgCKAMgAAAP8AAAIBgCaAMgAAAP8AAAMCgCqAMgAAAP8AAAkBAwQEAAAQAgIFAgUIBAAAHAEBBwMHEBQAAQACAgkECRAaBAAUAQELBQscFAAAFAEBDQYNEgYAABQCAQ8HDxIKBAAQAQERCBEaAgAAIAEBEwkTGAoAABABARU=',
  /** The diff of those five ticks: one agent walking. 35 bytes. */
  diff_real:
    'T0dXMQIBAQbwwIjzqbXkwuYBBwAAAQEXgCKELwAEAQEAAAA=',
} as const;

function fixture(name: keyof typeof FIXTURES): Uint8Array {
  return new Uint8Array(Buffer.from(FIXTURES[name].replace(/\s+/g, ''), 'base64'));
}

/** Test-only writer: the malformed-blob factory, mirroring `binary.rs`. */
class W {
  bytes: number[] = [];
  header(kind: number, version = 1): W {
    this.bytes.push(0x4f, 0x47, 0x57, 0x31, kind, version);
    return this;
  }
  u8(v: number): W {
    this.bytes.push(v & 0xff);
    return this;
  }
  varint(v: number): W {
    let n = v;
    for (;;) {
      const b = n % 128;
      n = Math.floor(n / 128);
      if (n === 0) {
        this.bytes.push(b);
        return this;
      }
      this.bytes.push(b | 0x80);
    }
  }
  zigzag(v: number): W {
    return this.varint(((v << 1) ^ (v >> 31)) >>> 0);
  }
  strings(items: string[]): W {
    this.varint(items.length);
    for (const s of items) {
      const raw = Buffer.from(s, 'utf-8');
      this.varint(raw.length);
      for (const b of raw) this.bytes.push(b);
    }
    return this;
  }
  raw(...bytes: number[]): W {
    this.bytes.push(...bytes);
    return this;
  }
  done(): Uint8Array {
    return new Uint8Array(this.bytes);
  }
}

/** An empty but valid diff, as the base of the malformed cases. */
function emptyDiff(): W {
  return new W()
    .header(KIND_DIFF)
    .varint(1) // from
    .varint(2) // to
    .varint(0) // map hash
    .varint(0) // rng
    .u8(0) // sleep
    .strings([]);
}

function code(fn: () => unknown): string {
  try {
    fn();
  } catch (e) {
    return e instanceof CodecError ? e.code : `not a CodecError: ${String(e)}`;
  }
  return 'no error';
}

describe('codec: snapshots from the Rust crate', () => {
  it('decodes the hand-built sample byte for byte', () => {
    const s = decodeSnapshot(fixture('snapshot_sample'));
    expect(s.tick).toBe(1234);
    expect(s.seed).toBe(99n);
    expect(s.rngState).toBe(0xdeadbeefn);
    expect(s.mapHash).toBe(0x0123456789abcdefn);
    expect(s.sleep).toBe('dozing');
    expect(s.agents).toHaveLength(2);
    expect(s.agents[0]).toEqual({
      id: 1,
      name: 'Omni',
      x: 640,
      y: -128,
      z: 0,
      dir: 3,
      anim: 1,
      energy: 200,
      activity: { tag: 1, name: 'walking', arg: 0 },
    });
    expect(s.agents[1]).toEqual({
      id: 2,
      name: 'Ada',
      x: 1,
      y: 2,
      z: 256,
      dir: 0,
      anim: 4,
      energy: 10,
      activity: { tag: 4, name: 'working', arg: 7 },
    });
    expect(s.objects).toEqual([
      {
        id: 7,
        kind: 'object/workbench',
        tx: 4,
        ty: -4,
        dir: 2,
        walkable: false,
        height: 20,
        footprint: [2, 1],
        slot: 'study-bench',
      },
    ]);
  });

  it('negative coordinates survive the zigzag', () => {
    const s = decodeSnapshot(fixture('snapshot_sample'));
    expect(toTiles(s.agents[0].x)).toBe(2.5);
    expect(toTiles(s.agents[0].y)).toBe(-0.5);
    expect(s.objects[0].ty).toBe(-4);
  });

  it('decodes a snapshot of a real World with its house', () => {
    const s = decodeSnapshot(fixture('snapshot_real'));
    expect(s.tick).toBe(1);
    expect(s.sleep).toBe('active');
    expect(s.mapHash).not.toBe(0n);
    expect(s.agents.map((a) => a.name)).toEqual(['Omni', 'Ada', 'Rex']);
    expect(s.agents.map((a) => a.id)).toEqual([1, 2, 3]);
    expect(s.agents.every((a) => a.energy === 255)).toBe(true);
    expect(s.objects).toHaveLength(9);
    expect(s.objects.map((o) => o.id)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9]);
    // Every object of house-v1.min sits in a named slot, and the workbench is
    // the 2x1 one.
    expect(s.objects.every((o) => o.slot !== null)).toBe(true);
    const bench = s.objects.find((o) => o.kind === 'object/workbench');
    expect(bench?.footprint).toEqual([2, 1]);
    expect(bench?.walkable).toBe(false);
    expect(bench?.slot).toBe('study-bench');
    // Ids are ascending, which the state layer relies on.
    for (let i = 1; i < s.agents.length; i++) {
      expect(s.agents[i].id).toBeGreaterThan(s.agents[i - 1].id);
    }
  });
});

describe('codec: diffs from the Rust crate', () => {
  it('reads every field mask of the sample', () => {
    const d = decodeDiff(fixture('diff_sample'));
    expect(d.from).toBe(10);
    expect(d.to).toBe(11);
    expect(d.sleep).toBe('active');
    expect(d.rngState).toBe(7n);
    expect(d.ents).toHaveLength(4);

    const moved = d.ents[0];
    expect(moved.mask).toBe(FIELD_POS | FIELD_DIR | FIELD_ANIM);
    expect([moved.x, moved.y, moved.z]).toEqual([-300, 512, 0]);
    expect(moved.dir).toBe(6);
    expect(moved.name).toBeNull();

    const spawned = d.ents[1];
    expect(spawned.mask).toBe(FIELD_FULL);
    expect(spawned.name).toBe('Ada');
    expect(spawned.energy).toBe(255);

    const sat = d.ents[2];
    expect(sat.mask).toBe(FIELD_ENERGY | FIELD_ACT);
    expect(sat.energy).toBe(12);
    expect(sat.activity).toEqual({ tag: 2, name: 'sitting', arg: 9 });
    // A field the mask does not claim stays at its default, exactly as in Rust.
    expect([sat.x, sat.y, sat.z, sat.dir, sat.anim]).toEqual([0, 0, 0, 0, 0]);

    expect(d.ents[3].mask).toBe(FIELD_DESPAWNED);
  });

  it('reads both object ops', () => {
    const d = decodeDiff(fixture('diff_sample'));
    expect(d.objects).toHaveLength(2);
    expect(d.objects[0]).toEqual({ op: 'removed', id: 3 });
    expect(d.objects[1]).toEqual({
      op: 'placed',
      id: 5,
      object: {
        id: 5,
        kind: 'object/lamp',
        tx: -2,
        ty: 9,
        dir: 4,
        walkable: true,
        height: 28,
        footprint: [1, 1],
        slot: null,
      },
    });
  });

  it('reads all thirteen event tags, including the non-ASCII text', () => {
    const d = decodeDiff(fixture('diff_sample'));
    expect(d.events.map((e) => e.type)).toEqual([
      'arrived',
      'started_anim',
      'said',
      'interacted',
      'slept',
      'woke',
      'yawned',
      'spawned',
      'despawned',
      'object_placed',
      'object_removed',
      'caught_up',
      'rejected',
    ]);
    expect(d.events[0]).toEqual({ type: 'arrived', ent: 1, tx: 3, ty: -7 });
    expect(d.events[2]).toEqual({ type: 'said', ent: 2, text: 'olá, mundo' });
    expect(d.events[11]).toEqual({ type: 'caught_up', ticks: 2880 });
    expect(d.events[12]).toEqual({
      type: 'rejected',
      ent: 1,
      code: 'ERR_WORLD_UNREACHABLE',
    });
  });

  it('decodes the diff a real World produced over five ticks', () => {
    const d = decodeDiff(fixture('diff_real'));
    expect(d.from).toBe(1);
    expect(d.to).toBe(6);
    expect(d.ents).toHaveLength(1);
    expect(d.ents[0].id).toBe(1);
    expect(d.ents[0].mask & FIELD_POS).toBeTruthy();
    expect(d.objects).toHaveLength(0);
  });

  it('64 agents moving stay well inside the 2 KB budget line', () => {
    const bytes = fixture('diff_64');
    expect(bytes.length).toBe(924);
    const d = decodeDiff(bytes);
    expect(d.ents).toHaveLength(64);
    expect(d.events).toHaveLength(64);
  });

  it('decodeBlob tells the two kinds apart', () => {
    expect(decodeBlob(fixture('snapshot_sample')).kind).toBe('snapshot');
    expect(decodeBlob(fixture('diff_sample')).kind).toBe('diff');
  });

  it('a diff blob is refused by the snapshot decoder and vice versa', () => {
    expect(code(() => decodeSnapshot(fixture('diff_sample')))).toBe(CODEC_ERR.BAD_KIND);
    expect(code(() => decodeDiff(fixture('snapshot_sample')))).toBe(CODEC_ERR.BAD_KIND);
  });
});

describe('codec: a malformed blob is an error with a code, never a shrug', () => {
  it('checks magic, version and kind', () => {
    const good = fixture('snapshot_sample');
    const badMagic = good.slice();
    badMagic[0] = 0x58;
    expect(code(() => decodeSnapshot(badMagic))).toBe(CODEC_ERR.BAD_MAGIC);

    const badVersion = good.slice();
    badVersion[5] = 9;
    expect(code(() => decodeSnapshot(badVersion))).toBe(CODEC_ERR.BAD_VERSION);

    const badKind = good.slice();
    badKind[4] = 7;
    expect(code(() => decodeSnapshot(badKind))).toBe(CODEC_ERR.BAD_KIND);

    expect(code(() => decodeSnapshot(good.slice(0, HEADER_LEN - 1)))).toBe(CODEC_ERR.TRUNCATED);
  });

  it('every truncation of every fixture throws instead of returning junk', () => {
    for (const name of ['snapshot_sample', 'diff_sample', 'snapshot_real', 'diff_real'] as const) {
      const bytes = fixture(name);
      const decode = name.startsWith('snapshot') ? decodeSnapshot : decodeDiff;
      for (let cut = 0; cut < bytes.length; cut++) {
        expect(() => decode(bytes.slice(0, cut)), `${name} cut at ${cut}`).toThrow(CodecError);
      }
    }
  });

  it('refuses bytes left over at the end', () => {
    const bytes = fixture('snapshot_sample');
    const longer = new Uint8Array(bytes.length + 1);
    longer.set(bytes);
    expect(code(() => decodeSnapshot(longer))).toBe(CODEC_ERR.TRAILING);
  });

  it('refuses a varint that never ends', () => {
    const w = new W().header(KIND_SNAPSHOT);
    for (let i = 0; i < 12; i++) w.u8(0xff);
    expect(code(() => decodeSnapshot(w.done()))).toBe(CODEC_ERR.VARINT);
  });

  it('refuses an absurd count before allocating for it', () => {
    const w = emptyDiff().varint(0xffffffff);
    expect(code(() => decodeDiff(w.done()))).toBe(CODEC_ERR.TOO_LONG);
  });

  it('refuses an unknown sleep state, activity, animation and event tag', () => {
    const sleep = new W()
      .header(KIND_SNAPSHOT)
      .varint(0)
      .varint(0)
      .varint(0)
      .varint(0)
      .u8(9)
      .strings([]);
    expect(code(() => decodeSnapshot(sleep.done()))).toBe(CODEC_ERR.BAD_TAG);

    const badAct = new W()
      .header(KIND_SNAPSHOT)
      .varint(0)
      .varint(0)
      .varint(0)
      .varint(0)
      .u8(0)
      .strings(['Omni'])
      .varint(1) // one agent
      .varint(1) // id
      .varint(0) // name index
      .zigzag(0)
      .zigzag(0)
      .zigzag(0)
      .u8(0) // dir
      .u8(0) // anim
      .u8(255) // energy
      .u8(99) // activity tag: nonsense
      .varint(0)
      .varint(0);
    expect(code(() => decodeSnapshot(badAct.done()))).toBe(CODEC_ERR.BAD_TAG);

    const badAnim = emptyDiff()
      .varint(1)
      .varint(1)
      .u8(FIELD_ANIM)
      .u8(99)
      .varint(0)
      .varint(0);
    expect(code(() => decodeDiff(badAnim.done()))).toBe(CODEC_ERR.BAD_TAG);

    const badEvent = emptyDiff().varint(0).varint(0).varint(1).u8(77);
    expect(code(() => decodeDiff(badEvent.done()))).toBe(CODEC_ERR.BAD_TAG);
  });

  it('refuses a despawn mask carrying any other bit', () => {
    const w = emptyDiff()
      .varint(1)
      .varint(1)
      .u8(FIELD_DESPAWNED | FIELD_POS)
      .zigzag(0)
      .zigzag(0)
      .zigzag(0)
      .varint(0)
      .varint(0);
    expect(code(() => decodeDiff(w.done()))).toBe(CODEC_ERR.BAD_TAG);
  });

  it('refuses an unknown object op', () => {
    const w = emptyDiff().varint(0).varint(1).varint(4).u8(9);
    expect(code(() => decodeDiff(w.done()))).toBe(CODEC_ERR.BAD_TAG);
  });

  it('refuses invalid UTF-8 and a string index out of range', () => {
    const invalid = new W()
      .header(KIND_SNAPSHOT)
      .varint(0)
      .varint(0)
      .varint(0)
      .varint(0)
      .u8(0)
      .varint(1)
      .varint(2)
      .raw(0xff, 0xfe);
    expect(code(() => decodeSnapshot(invalid.done()))).toBe(CODEC_ERR.BAD_STRING);

    const outOfRange = emptyDiff()
      .varint(1)
      .varint(1)
      .u8(FIELD_SPAWNED)
      .varint(3) // name index into an empty table
      .varint(0)
      .varint(0);
    expect(code(() => decodeDiff(outOfRange.done()))).toBe(CODEC_ERR.BAD_STRING);
  });

  it('a u64 identity survives the way a double would not', () => {
    // 2^63 + 1 as map_hash: a number would round it, a bigint does not.
    const big = 0x8000000000000001n;
    const w = new W().header(KIND_SNAPSHOT).varint(0).varint(0).varint(0);
    let v = big;
    for (;;) {
      const b = Number(v & 0x7fn);
      v >>= 7n;
      if (v === 0n) {
        w.u8(b);
        break;
      }
      w.u8(b | 0x80);
    }
    w.u8(0).strings([]).varint(0).varint(0);
    expect(decodeSnapshot(w.done()).mapHash).toBe(big);
  });

  it('the activity names cover the eight tags in order', () => {
    expect(ACTIVITY_NAMES).toHaveLength(8);
    expect(ACTIVITY_NAMES[0]).toBe('idle');
    expect(ACTIVITY_NAMES[7]).toBe('yawning');
  });
});
