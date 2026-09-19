import { describe, expect, it } from 'vitest';
import {
  ASK_QUEUE_MAX,
  TOOL_NAME_MAX,
  acceptAsk,
  cancelRequest,
  headAsk,
  pushAsk,
  resolveAsk,
  sanitizeAsk,
  terminalRequestId,
  type PendingAsk,
} from './tool-ask-queue';

function ask(id: string, tool = 'fs.read', requestId = 'r1'): PendingAsk {
  return { agent: 'omni', requestId, toolCallId: id, tool };
}

describe('sanitizeAsk', () => {
  it('accepts the snake_case payload of llm://tool-ask', () => {
    expect(
      sanitizeAsk({ agent: 'omni', request_id: 'r1', tool_call_id: 'c1', tool: 'fs.read' })
    ).toEqual({ agent: 'omni', requestId: 'r1', toolCallId: 'c1', tool: 'fs.read' });
  });

  it('keeps only the four contract fields, so arguments cannot reach the bubble', () => {
    const clean = sanitizeAsk({
      agent: 'omni',
      request_id: 'r1',
      tool_call_id: 'c1',
      tool: 'fs.read',
      input: { path: '/Users/tonho/.ssh/id_ed25519' },
      args: 'secret',
    });
    expect(Object.keys(clean ?? {}).sort()).toEqual(['agent', 'requestId', 'tool', 'toolCallId']);
    expect(JSON.stringify(clean)).not.toContain('id_ed25519');
  });

  it('rejects a question it could never answer', () => {
    expect(sanitizeAsk(null)).toBeNull();
    expect(sanitizeAsk('fs.read')).toBeNull();
    expect(sanitizeAsk([])).toBeNull();
    expect(sanitizeAsk({ request_id: 'r1', tool: 'fs.read' })).toBeNull();
    expect(sanitizeAsk({ tool_call_id: 'c1', tool: 'fs.read' })).toBeNull();
    expect(sanitizeAsk({ request_id: 'r1', tool_call_id: 'c1' })).toBeNull();
    expect(sanitizeAsk({ request_id: 'r1', tool_call_id: 'c1', tool: '   ' })).toBeNull();
  });

  it('strips control characters and clips a name that is not a name', () => {
    const clean = sanitizeAsk({
      request_id: 'r1',
      tool_call_id: 'c1',
      tool: `  fs.\nread  `,
    });
    expect(clean?.tool).toBe('fs.read');
    const long = sanitizeAsk({ request_id: 'r1', tool_call_id: 'c1', tool: 'x'.repeat(500) });
    expect(long?.tool).toHaveLength(TOOL_NAME_MAX);
  });

  it('tolerates a missing agent', () => {
    expect(sanitizeAsk({ request_id: 'r1', tool_call_id: 'c1', tool: 't' })?.agent).toBe('');
  });
});

describe('the queue', () => {
  it('is FIFO: the head is the oldest question', () => {
    let q = pushAsk([], ask('c1', 'fs.read'));
    q = pushAsk(q, ask('c2', 'shell.run'));
    q = pushAsk(q, ask('c3', 'net.fetch'));
    expect(headAsk(q)?.toolCallId).toBe('c1');
    q = resolveAsk(q, 'c1');
    expect(headAsk(q)?.toolCallId).toBe('c2');
    q = resolveAsk(q, 'c2');
    q = resolveAsk(q, 'c3');
    expect(headAsk(q)).toBeNull();
  });

  it('never queues the same tool_call_id twice', () => {
    let q = pushAsk([], ask('c1'));
    q = pushAsk(q, ask('c1', 'shell.run'));
    expect(q).toHaveLength(1);
    expect(q[0].tool).toBe('fs.read');
  });

  it('answers a question that is gone without complaining (idempotent)', () => {
    const q = pushAsk([], ask('c1'));
    const once = resolveAsk(q, 'c1');
    expect(once).toEqual([]);
    expect(resolveAsk(once, 'c1')).toEqual([]);
    expect(resolveAsk([], 'never-existed')).toEqual([]);
  });

  it('drops a resolution that names another question', () => {
    const q = pushAsk(pushAsk([], ask('c1')), ask('c2'));
    expect(resolveAsk(q, 'c9')).toHaveLength(2);
  });

  it('cancels every question of a turn and leaves the others', () => {
    let q = pushAsk([], ask('c1', 'fs.read', 'r1'));
    q = pushAsk(q, ask('c2', 'shell.run', 'r1'));
    q = pushAsk(q, ask('c3', 'net.fetch', 'r2'));
    const left = cancelRequest(q, 'r1');
    expect(left.map((a) => a.toolCallId)).toEqual(['c3']);
    expect(cancelRequest(left, 'r1')).toEqual(left);
  });

  it('is bounded: a looping turn cannot grow it without end', () => {
    let q: PendingAsk[] = [];
    for (let i = 0; i < ASK_QUEUE_MAX + 20; i += 1) q = pushAsk(q, ask(`c${i}`));
    expect(q).toHaveLength(ASK_QUEUE_MAX);
    // The head is still the first question asked: the user answers in order.
    expect(headAsk(q)?.toolCallId).toBe('c0');
  });

  it('never mutates the array it was given', () => {
    const q = [ask('c1')];
    const frozen = Object.freeze([...q]);
    expect(() => pushAsk(frozen, ask('c2'))).not.toThrow();
    expect(() => resolveAsk(frozen, 'c1')).not.toThrow();
    expect(frozen).toHaveLength(1);
  });
});

describe('acceptAsk', () => {
  it('queues a raw payload and keeps the identity of the array on garbage', () => {
    const q: PendingAsk[] = [];
    const next = acceptAsk(q, { request_id: 'r1', tool_call_id: 'c1', tool: 'fs.read' });
    expect(next).toHaveLength(1);
    expect(acceptAsk(next, { nope: true })).toBe(next);
    // A duplicate changes nothing, so the window skips the re-render.
    expect(acceptAsk(next, { request_id: 'r1', tool_call_id: 'c1', tool: 'fs.read' })).toBe(next);
  });
});

describe('terminalRequestId', () => {
  it('spots the events that end a turn', () => {
    expect(terminalRequestId({ request_id: 'r1', event: { type: 'finished', reason: 'stop' } })).toBe(
      'r1'
    );
    expect(terminalRequestId({ request_id: 'r1', event: { type: 'error', error: {} } })).toBe('r1');
    // A cancelled turn still finishes (coordinator sends Finished(Cancelled)).
    expect(
      terminalRequestId({ request_id: 'r1', event: { type: 'finished', reason: 'cancelled' } })
    ).toBe('r1');
  });

  it('ignores everything else', () => {
    expect(terminalRequestId({ request_id: 'r1', event: { type: 'text_delta', text: 'hi' } })).toBeNull();
    expect(terminalRequestId({ request_id: 'r1' })).toBeNull();
    expect(terminalRequestId({ event: { type: 'finished' } })).toBeNull();
    expect(terminalRequestId(null)).toBeNull();
    expect(terminalRequestId('finished')).toBeNull();
  });

  it('survives an externally tagged serialisation it does not own', () => {
    expect(terminalRequestId({ request_id: 'r1', event: { Finished: { reason: 'stop' } } })).toBe('r1');
    expect(terminalRequestId({ request_id: 'r1', event: 'finished' })).toBe('r1');
  });
});
