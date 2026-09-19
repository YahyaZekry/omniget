import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { fakeTurnEvents, splitChunks } from "$lib/llm/fake-turn";
import type { TurnEvent } from "$lib/llm/types";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

type LlmStore = typeof import("./llm-store.svelte");

let store: LlmStore;

/** Frames are run by hand so the buffer can be inspected mid-turn. */
let frames: (() => void)[] = [];
function runFrame(): void {
  const next = frames.shift();
  next?.();
}

beforeAll(async () => {
  // The stub backend makes `sendMessage` play a fake turn on a timer; with fake
  // timers it never fires unless a test asks, so each test drives the stream.
  vi.useFakeTimers();
  vi.stubGlobal("$state", <T>(value: T) => value);
  vi.stubGlobal("$derived", <T>(value: T) => value);
  store = await import("./llm-store.svelte");
});

afterEach(() => {
  store.resetLlmStore();
  store.setFrameScheduler(null);
  invoke.mockReset();
  frames = [];
});

afterAll(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

function manualScheduler(run: () => void): () => void {
  frames.push(run);
  return () => {
    frames = frames.filter((f) => f !== run);
  };
}

async function startTurn(): Promise<string> {
  store.setFrameScheduler(manualScheduler);
  invoke.mockImplementation((cmd: string) => {
    if (cmd === "llm_roster_list") return Promise.reject("ERR_STUB");
    return Promise.reject("ERR_STUB");
  });
  await store.loadRoster();
  const agentId = store.getAgents()[0].id;
  store.selectAgent(agentId);
  await store.sendMessage("hi");
  const turn = store.getActiveTurn();
  expect(turn).not.toBeNull();
  return turn!.requestId;
}

describe("roster", () => {
  it("falls back to the demo roster on ERR_STUB and keeps the section usable", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    await store.loadRoster();
    expect(store.getAgents().length).toBeGreaterThan(0);
    expect(store.isDemoRoster()).toBe(true);
    expect(store.isRosterAvailable()).toBe(false);
  });

  it("uses the backend roster when it answers", async () => {
    const agent = {
      id: "a1",
      name: "Solo",
      role: "coordinator" as const,
      system_prompt: "",
      model: { policy: "fixed" as const, model: { provider: "openai", model: "gpt-5" } },
      runtime: { kind: "native" as const },
    };
    invoke.mockResolvedValue([agent]);
    await store.loadRoster();
    expect(store.getAgents()).toEqual([agent]);
    expect(store.isDemoRoster()).toBe(false);
    expect(store.isRosterAvailable()).toBe(true);
  });

  it("treats the harness `null` answer as unavailable, not as a crash", async () => {
    invoke.mockResolvedValue(null);
    await store.loadRoster();
    expect(store.isDemoRoster()).toBe(true);
    expect(store.getAgents().length).toBeGreaterThan(0);
  });
});

describe("turn buffer", () => {
  it("applies deltas in wire order in a single frame", async () => {
    const requestId = await startTurn();
    const events: TurnEvent[] = [
      { type: "started", request_id: requestId },
      { type: "text_delta", text: "Hel" },
      { type: "tool_call_start", id: "c1", name: "downloads.list" },
      { type: "tool_call_delta", id: "c1", input_json_delta: '{"a":' },
      { type: "text_delta", text: "lo " },
      { type: "tool_call_delta", id: "c1", input_json_delta: "1}" },
      { type: "tool_call_end", id: "c1" },
      { type: "text_delta", text: "world" },
    ];
    for (const event of events) store.handleTurnEvent(requestId, event);
    expect(store.pendingEventCount()).toBe(events.length);
    expect(store.scheduledFrameCount()).toBe(1);
    runFrame();
    const turn = store.getActiveTurn()!;
    expect(turn.text).toBe("Hello world");
    expect(turn.starting).toBe(false);
    expect(turn.toolCalls).toEqual([
      { id: "c1", name: "downloads.list", input: '{"a":1}', done: true },
    ]);
    expect(store.pendingEventCount()).toBe(0);
  });

  it("ignores events of another request id", async () => {
    const requestId = await startTurn();
    store.handleTurnEvent("someone-else", { type: "text_delta", text: "nope" });
    expect(store.pendingEventCount()).toBe(0);
    store.handleTurnEvent(requestId, { type: "text_delta", text: "yes" });
    runFrame();
    expect(store.getActiveTurn()!.text).toBe("yes");
  });

  it("schedules no frame at rest and drops it when the turn ends", async () => {
    const requestId = await startTurn();
    expect(store.scheduledFrameCount()).toBe(0);
    store.handleTurnEvent(requestId, { type: "text_delta", text: "x" });
    expect(store.scheduledFrameCount()).toBe(1);
    runFrame();
    expect(store.scheduledFrameCount()).toBe(0);
    store.handleTurnEvent(requestId, { type: "finished", reason: "stop" });
    runFrame();
    expect(store.isTurnRunning()).toBe(false);
    expect(store.scheduledFrameCount()).toBe(0);
    expect(store.pendingEventCount()).toBe(0);
  });

  it("commits the assistant message with usage and the model label", async () => {
    const requestId = await startTurn();
    for (const event of fakeTurnEvents(requestId, { answer: "done", chunks: 2 })) {
      store.handleTurnEvent(requestId, event);
    }
    runFrame();
    const messages = store.getMessages();
    expect(messages.map((m) => m.role)).toEqual(["user", "assistant"]);
    const last = messages[1];
    expect(last.text).toBe("done");
    expect(last.usage?.input_tokens).toBe(412);
    expect(last.modelLabel).toBe("anthropic/claude-opus-5");
    expect(last.finish).toBe("stop");
  });
});

describe("cancellation", () => {
  it("clears turn, buffer and frame, and keeps what streamed so far", async () => {
    const requestId = await startTurn();
    store.handleTurnEvent(requestId, { type: "text_delta", text: "partial" });
    runFrame();
    store.handleTurnEvent(requestId, { type: "text_delta", text: " more" });
    expect(store.pendingEventCount()).toBe(1);
    await store.cancelTurn();
    expect(store.isTurnRunning()).toBe(false);
    expect(store.pendingEventCount()).toBe(0);
    expect(store.scheduledFrameCount()).toBe(0);
    const messages = store.getMessages();
    expect(messages[messages.length - 1].finish).toBe("cancelled");
    expect(messages[messages.length - 1].text).toBe("partial more");
  });

  it("is a no-op with no turn running", async () => {
    await expect(store.cancelTurn()).resolves.toBeUndefined();
    expect(store.scheduledFrameCount()).toBe(0);
  });
});

describe("tool ask (GrantMode::Ask)", () => {
  it("keeps the prompt of the live request and clears it on the answer", async () => {
    const requestId = await startTurn();
    store.handleToolAsk({
      agent: "omni",
      request_id: "another-turn",
      tool_call_id: "c1",
      tool: "downloads.enqueue",
    });
    expect(store.getToolAsk()).toBeNull();
    store.handleToolAsk({
      agent: "omni",
      request_id: requestId,
      tool_call_id: "c1",
      tool: "downloads.enqueue",
    });
    expect(store.getToolAsk()?.tool).toBe("downloads.enqueue");
    await store.answerTool("c1", true);
    expect(store.getToolAsk()).toBeNull();
    expect(invoke).toHaveBeenCalledWith("llm_tool_answer", {
      requestId,
      toolCallId: "c1",
      allow: true,
      always: false,
    });
  });

  it("drops a pending prompt when the turn ends", async () => {
    const requestId = await startTurn();
    store.handleToolAsk({ agent: "omni", request_id: requestId, tool_call_id: "c2", tool: "tools.run" });
    store.handleTurnEvent(requestId, { type: "finished", reason: "stop" });
    runFrame();
    expect(store.getToolAsk()).toBeNull();
    expect(store.isTurnRunning()).toBe(false);
  });
});

describe("conversations", () => {
  it("reuses the agent's conversation and keeps the model override", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    await store.loadRoster();
    const [first, second] = store.getAgents();
    const a = store.selectAgent(first.id);
    store.selectAgent(second.id);
    expect(store.selectAgent(first.id)).toBe(a);
    await store.switchModel({ provider: "openai", model: "gpt-5" });
    expect(store.getActiveConversation()!.model).toEqual({ provider: "openai", model: "gpt-5" });
  });
});

describe("fake turn helpers", () => {
  it("splits without breaking surrogate pairs", () => {
    expect(splitChunks("abcdef", 3)).toEqual(["ab", "cd", "ef"]);
    expect(splitChunks("🙂🙂", 2)).toEqual(["🙂", "🙂"]);
    expect(splitChunks("", 4)).toEqual([""]);
  });

  it("scripts started → deltas → usage → finished", () => {
    const events = fakeTurnEvents("r1", { answer: "hey", chunks: 3 });
    expect(events[0]).toEqual({ type: "started", request_id: "r1" });
    expect(events.filter((e) => e.type === "text_delta")).toHaveLength(3);
    expect(events[events.length - 2].type).toBe("usage");
    expect(events[events.length - 1]).toEqual({ type: "finished", reason: "stop" });
  });
});
