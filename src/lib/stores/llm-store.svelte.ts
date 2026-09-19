/**
 * State of the `/llm` section: roster, conversations and the live turn.
 *
 * Three rules shape this file:
 *
 * 1. **Nothing runs at rest.** The `llm://turn` listener is attached when a
 *    turn starts and dropped when it ends; the delta buffer only schedules a
 *    frame while there are events waiting. With no turn there is no listener,
 *    no timer and no `requestAnimationFrame`.
 * 2. **Deltas are applied in wire order.** Events land in a FIFO buffer and a
 *    single frame drains it; a `text_delta` never overtakes a `tool_call_end`.
 * 3. **`ERR_STUB` is not a failure.** While the Rust side is a stub every
 *    command answers `ERR_STUB` (and the screenshot harness answers `null`):
 *    the store then shows the demo roster from `$lib/llm/fake-turn` and runs
 *    the turn locally, so the UI is exercisable before the backend exists.
 */
import { invoke } from "@tauri-apps/api/core";
import {
  DEMO_ANSWER,
  DEMO_ROSTER,
  fakeTurnEvents,
  runFakeTurn,
  type CancelFake,
} from "$lib/llm/fake-turn";
import type {
  AgentDef,
  ChatMessage,
  Conversation,
  LlmError,
  ModelRef,
  ToolAsk,
  TurnEvent,
  TurnEventEnvelope,
  TurnState,
} from "$lib/llm/types";
import { effectiveModelLabel } from "$lib/llm/types";

// ── Frame scheduler (seam: vitest runs in node, with no rAF) ────────────

export type FrameScheduler = (run: () => void) => () => void;

function defaultScheduler(run: () => void): () => void {
  if (typeof requestAnimationFrame === "function") {
    // A hidden or covered window gets no frames; the timer keeps the turn
    // moving (and finishing) while the user is in another app.
    let done = false;
    const once = () => {
      if (done) return;
      done = true;
      cancelAnimationFrame(frame);
      clearTimeout(timer);
      run();
    };
    const frame = requestAnimationFrame(once);
    const timer = setTimeout(once, 250);
    return () => {
      done = true;
      cancelAnimationFrame(frame);
      clearTimeout(timer);
    };
  }
  const id = setTimeout(run, 16);
  return () => clearTimeout(id);
}

let scheduler: FrameScheduler = defaultScheduler;

/** Test seam. `null` restores the real one. */
export function setFrameScheduler(next: FrameScheduler | null): void {
  scheduler = next ?? defaultScheduler;
}

// ── State ───────────────────────────────────────────────────────────────

let agents = $state<AgentDef[]>([]);
let rosterLoading = $state(false);
let rosterAvailable = $state(true);
let demoRoster = $state(false);
let conversations = $state<Conversation[]>([]);
let activeConversationId = $state<string | null>(null);
let turn = $state<TurnState | null>(null);
let toolAsk = $state<ToolAsk | null>(null);
let errorKey = $state<string | null>(null);

let rosterLoadedOnce = false;
let rosterInFlight: Promise<void> | null = null;
let pending: TurnEvent[] = [];
let frameCancel: (() => void) | null = null;
let cancelFake: CancelFake | null = null;
let unlisten: (() => void) | null = null;
let idCounter = 0;

function nextId(prefix: string): string {
  idCounter += 1;
  return `${prefix}-${Date.now().toString(36)}-${idCounter}`;
}

// ── Roster ──────────────────────────────────────────────────────────────

export function getAgents(): AgentDef[] {
  return agents;
}

export function getAgent(id: string | null | undefined): AgentDef | null {
  if (!id) return null;
  return agents.find((a) => a.id === id) ?? null;
}

export function isRosterLoading(): boolean {
  return rosterLoading;
}

/** False once a command answered `ERR_STUB`. */
export function isRosterAvailable(): boolean {
  return rosterAvailable;
}

/** True when the rail is showing `DEMO_ROSTER` instead of saved agents. */
export function isDemoRoster(): boolean {
  return demoRoster;
}

export function getErrorKey(): string | null {
  return errorKey;
}

export function isUnavailable(err: unknown): boolean {
  return String(err ?? "").includes("ERR_STUB");
}

/** Maps a backend error to an i18n key. */
export function llmErrorKey(err: unknown): string {
  const code = String(err ?? "");
  if (code.includes("ERR_STUB")) return "llm.err.unavailable";
  if (code.includes("ERR_LLM_BUDGET")) return "llm.err.budget";
  if (code.includes("ERR_LLM")) return "llm.err.turn_failed";
  return "llm.err.turn_failed";
}

/** Loads once per session unless `force`. Never throws. */
export function loadRoster(force = false): Promise<void> {
  if (rosterInFlight) return rosterInFlight;
  if (rosterLoadedOnce && !force) return Promise.resolve();
  rosterLoading = true;
  rosterInFlight = invoke<AgentDef[] | null>("llm_roster_list")
    .then((list) => {
      if (Array.isArray(list) && list.length > 0) {
        agents = list;
        rosterAvailable = true;
        demoRoster = false;
        return;
      }
      // `null` is what the screenshot harness answers for an unknown command.
      agents = DEMO_ROSTER;
      demoRoster = true;
      rosterAvailable = Array.isArray(list);
    })
    .catch((err) => {
      agents = DEMO_ROSTER;
      demoRoster = true;
      rosterAvailable = !isUnavailable(err);
      if (!isUnavailable(err)) errorKey = llmErrorKey(err);
    })
    .finally(() => {
      rosterLoading = false;
      rosterLoadedOnce = true;
      rosterInFlight = null;
      if (!activeConversationId && agents.length > 0) selectAgent(agents[0].id);
    });
  return rosterInFlight;
}

/**
 * Puts one canned exchange in front of the user while the backend is a stub,
 * so the chat shows what it will look like instead of an empty column. Called
 * by the page, never by `loadRoster`, and only in demo mode.
 */
export function seedDemoConversation(): void {
  if (!demoRoster || agents.length === 0) return;
  if (conversations.some((c) => c.messages.length > 0)) return;
  const agent = agents[0];
  const id = selectAgent(agent.id);
  const conversation = conversations.find((c) => c.id === id);
  if (!conversation) return;
  conversation.title = "Retry the failed downloads";
  conversation.messages = [
    { id: nextId("msg"), role: "user", text: "Retry the failed downloads, please." },
    {
      id: nextId("msg"),
      role: "assistant",
      text: DEMO_ANSWER,
      toolCalls: [
        { id: "call-demo", name: "downloads.list", input: '{"status":"failed"}', done: true },
      ],
      usage: {
        input_tokens: 412,
        output_tokens: 96,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        first_token_ms: 180,
        total_ms: 1420,
        cost_usd: 0.0021,
      },
      modelLabel: effectiveModelLabel(conversation, agent),
      finish: "stop",
    },
  ];
}

/** Creates or updates one agent. Returns false when the backend refused. */
export async function saveAgent(agent: AgentDef): Promise<boolean> {
  const existing = agents.some((a) => a.id === agent.id);
  agents = existing
    ? agents.map((a) => (a.id === agent.id ? agent : a))
    : [...agents, agent];
  try {
    await invoke(existing ? "llm_roster_update" : "llm_roster_create", { agent });
    rosterAvailable = true;
    return true;
  } catch (err) {
    rosterAvailable = !isUnavailable(err);
    return false;
  }
}

export async function deleteAgent(agentId: string): Promise<boolean> {
  agents = agents.filter((a) => a.id !== agentId);
  conversations = conversations.filter((c) => c.agentId !== agentId);
  if (!conversations.some((c) => c.id === activeConversationId)) {
    activeConversationId = conversations[0]?.id ?? null;
  }
  try {
    await invoke("llm_roster_delete", { agentId });
    return true;
  } catch (err) {
    rosterAvailable = !isUnavailable(err);
    return false;
  }
}

export async function applyTemplate(template: string): Promise<boolean> {
  try {
    const list = await invoke<AgentDef[] | null>("llm_roster_apply_template", { template });
    if (Array.isArray(list)) {
      agents = list;
      rosterAvailable = true;
      demoRoster = false;
      return true;
    }
    return false;
  } catch (err) {
    rosterAvailable = !isUnavailable(err);
    return false;
  }
}

// ── Conversations ───────────────────────────────────────────────────────

export function getConversations(): Conversation[] {
  return conversations;
}

export function getActiveConversation(): Conversation | null {
  if (!activeConversationId) return null;
  return conversations.find((c) => c.id === activeConversationId) ?? null;
}

export function getActiveAgentId(): string | null {
  return getActiveConversation()?.agentId ?? null;
}

export function getMessages(): ChatMessage[] {
  return getActiveConversation()?.messages ?? [];
}

/** Last assistant line of an agent, for the rail preview. */
export function lastSpokenBy(agentId: string): string {
  for (let i = conversations.length - 1; i >= 0; i--) {
    const conversation = conversations[i];
    if (conversation.agentId !== agentId) continue;
    for (let j = conversation.messages.length - 1; j >= 0; j--) {
      const message = conversation.messages[j];
      if (message.text) return message.text;
    }
  }
  return "";
}

export function newConversation(agentId: string): string {
  const conversation: Conversation = {
    id: nextId("conv"),
    agentId,
    title: "",
    messages: [],
    model: null,
    updatedAtMs: Date.now(),
  };
  conversations = [...conversations, conversation];
  activeConversationId = conversation.id;
  return conversation.id;
}

/** Opens the agent's newest conversation, creating one when there is none. */
export function selectAgent(agentId: string): string {
  const existing = [...conversations].reverse().find((c) => c.agentId === agentId);
  if (existing) {
    activeConversationId = existing.id;
    return existing.id;
  }
  return newConversation(agentId);
}

export function selectConversation(id: string): void {
  if (conversations.some((c) => c.id === id)) activeConversationId = id;
}

/**
 * Undo took a turn back on disk: drop its bubbles (and everything after them)
 * here too. `userMessagesKept` is how many user messages the backend kept.
 */
export function rewindConversation(id: string, userMessagesKept: number): void {
  const conversation = conversations.find((c) => c.id === id);
  if (!conversation) return;
  let seen = 0;
  const cut = conversation.messages.findIndex((m) => m.role === "user" && ++seen > userMessagesKept);
  if (cut < 0) return;
  conversation.messages = conversation.messages.slice(0, cut);
  conversation.updatedAtMs = Date.now();
}

export function deleteConversation(id: string): void {
  conversations = conversations.filter((c) => c.id !== id);
  if (activeConversationId === id) activeConversationId = conversations[0]?.id ?? null;
}

/** Per-conversation model override (`llm_switch_model`). */
export async function switchModel(model: ModelRef): Promise<boolean> {
  const conversation = getActiveConversation();
  if (!conversation) return false;
  conversation.model = model;
  conversation.updatedAtMs = Date.now();
  try {
    await invoke("llm_switch_model", { conversationId: conversation.id, modelRef: model });
    return true;
  } catch (err) {
    rosterAvailable = !isUnavailable(err);
    return false;
  }
}

// ── Turn ────────────────────────────────────────────────────────────────

export function getActiveTurn(): TurnState | null {
  return turn;
}

/** Pending `GrantMode::Ask` prompt, or `null`. */
export function getToolAsk(): ToolAsk | null {
  return toolAsk;
}

/** Entry point for `llm://tool-ask`; exported so tests can feed it. */
export function handleToolAsk(ask: ToolAsk): void {
  if (!turn || turn.requestId !== ask.request_id) return;
  toolAsk = ask;
}

export function isTurnRunning(): boolean {
  return turn !== null;
}

/** Events waiting for the next frame. 0 at rest. */
export function pendingEventCount(): number {
  return pending.length;
}

/** 1 while a frame is scheduled, 0 at rest. The budget says 0 with no turn. */
export function scheduledFrameCount(): number {
  return frameCancel ? 1 : 0;
}

function enqueue(event: TurnEvent): void {
  pending.push(event);
  if (!frameCancel) frameCancel = scheduler(flushPending);
}

/** Drains the buffer in arrival order. Exported so tests can step the clock. */
export function flushPending(): void {
  frameCancel = null;
  if (pending.length === 0) return;
  const batch = pending;
  pending = [];
  for (const event of batch) applyEvent(event);
}

/** Entry point for both the Tauri event and the fake turn. */
export function handleTurnEvent(requestId: string, event: TurnEvent): void {
  if (!turn || turn.requestId !== requestId) return;
  enqueue(event);
}

function applyEvent(event: TurnEvent): void {
  const state = turn;
  if (!state) return;
  switch (event.type) {
    case "started":
      state.starting = false;
      break;
    case "text_delta":
      state.text += event.text;
      break;
    case "thinking_delta":
      state.thinking += event.text;
      break;
    case "tool_call_start":
      state.toolCalls = [...state.toolCalls, { id: event.id, name: event.name, input: "", done: false }];
      break;
    case "tool_call_delta": {
      const call = state.toolCalls.find((c) => c.id === event.id);
      if (call) call.input += event.input_json_delta;
      break;
    }
    case "tool_call_end": {
      const call = state.toolCalls.find((c) => c.id === event.id);
      if (call) call.done = true;
      break;
    }
    case "usage":
      state.usage = event.usage;
      break;
    case "error":
      state.error = event.error;
      break;
    case "finished":
      commitTurn(event.reason === "cancelled" ? "cancelled" : event.reason, state.error);
      break;
  }
}

function commitTurn(finish: ChatMessage["finish"], error: LlmError | null): void {
  const state = turn;
  if (!state) return;
  const conversation = conversations.find((c) => c.id === state.conversationId);
  if (conversation && (state.text || state.toolCalls.length > 0 || error)) {
    const agent = getAgent(state.agentId);
    conversation.messages = [
      ...conversation.messages,
      {
        id: nextId("msg"),
        role: "assistant",
        text: state.text,
        toolCalls: state.toolCalls.length > 0 ? state.toolCalls : undefined,
        thinking: state.thinking || undefined,
        usage: state.usage,
        modelLabel: effectiveModelLabel(conversation, agent),
        error,
        finish,
      },
    ];
    conversation.updatedAtMs = Date.now();
  }
  stopTurn();
}

/** Clears every resource a turn owns: buffer, frame, timer and listener. */
function stopTurn(): void {
  turn = null;
  toolAsk = null;
  pending = [];
  if (frameCancel) frameCancel();
  frameCancel = null;
  cancelFake = null;
  if (unlisten) unlisten();
  unlisten = null;
}

async function attachListener(): Promise<void> {
  if (unlisten) return;
  try {
    const { listen } = await import("@tauri-apps/api/event");
    const stopTurnEvents = await listen<TurnEventEnvelope>("llm://turn", (message) => {
      const payload = message.payload;
      if (payload?.request_id && payload.event) handleTurnEvent(payload.request_id, payload.event);
    });
    // `GrantMode::Ask`: the coordinator re-emits `BusEvent::ToolAsk` here.
    const stopAsks = await listen<ToolAsk>("llm://tool-ask", (message) => {
      if (message.payload?.tool_call_id) handleToolAsk(message.payload);
    });
    const stop = () => {
      stopTurnEvents();
      stopAsks();
    };
    // The turn may have ended while `listen` was in flight.
    if (turn) unlisten = stop;
    else stop();
  } catch {
    // No Tauri event bridge (browser, tests): the fake turn feeds the store.
  }
}

/**
 * Sends the user's text. Asks the backend first; when it answers `ERR_STUB`
 * (or nothing at all) the turn is played back locally so the UI still streams.
 */
export async function sendMessage(text: string): Promise<void> {
  const trimmed = text.trim();
  if (!trimmed || turn) return;
  let conversation = getActiveConversation();
  if (!conversation) {
    const agentId = agents[0]?.id;
    if (!agentId) return;
    selectAgent(agentId);
    conversation = getActiveConversation();
    if (!conversation) return;
  }
  conversation.messages = [
    ...conversation.messages,
    { id: nextId("msg"), role: "user", text: trimmed },
  ];
  if (!conversation.title) conversation.title = trimmed.slice(0, 48);
  conversation.updatedAtMs = Date.now();
  errorKey = null;

  const localRequestId = nextId("req");
  turn = {
    requestId: localRequestId,
    conversationId: conversation.id,
    agentId: conversation.agentId,
    text: "",
    thinking: "",
    toolCalls: [],
    usage: null,
    error: null,
    startedAtMs: Date.now(),
    starting: true,
  };

  let requestId: string | null = null;
  try {
    const answer = await invoke<string | { request_id?: string } | null>("llm_turn_start", {
      conversationId: conversation.id,
      agentId: conversation.agentId,
      input: trimmed,
    });
    if (typeof answer === "string") requestId = answer;
    else if (answer && typeof answer === "object" && answer.request_id) requestId = answer.request_id;
  } catch (err) {
    if (!isUnavailable(err)) {
      errorKey = llmErrorKey(err);
      rosterAvailable = false;
    }
  }

  if (!turn) return; // cancelled while the command was in flight
  if (requestId) {
    turn.requestId = requestId;
    void attachListener();
    return;
  }
  // Stub backend: local playback through the same event path.
  const events = fakeTurnEvents(localRequestId, {
    answer: DEMO_ANSWER,
    chunks: 40,
    thinking: "Checking the queue…",
    tool: { id: "call-1", name: "downloads.list", input: '{"status":"failed"}' },
  });
  cancelFake = runFakeTurn(events, (event) => handleTurnEvent(localRequestId, event), 24);
}

/** Stops the running turn: local timer first, then the backend. */
export async function cancelTurn(): Promise<void> {
  const state = turn;
  if (!state) return;
  const cancel = cancelFake;
  cancelFake = null;
  if (cancel) {
    cancel(); // emits `finished { cancelled }` through the buffer
    flushPending();
  } else {
    try {
      await invoke("llm_turn_cancel", { requestId: state.requestId });
    } catch {
      // Backend not wired: drop the turn locally anyway.
    }
    if (turn === state) {
      applyEvent({ type: "finished", reason: "cancelled" });
      if (turn === state) stopTurn();
    }
  }
}

/** Answers a tool permission prompt (`GrantMode::Ask`). */
export async function answerTool(toolCallId: string, allow: boolean, always = false): Promise<void> {
  const state = turn;
  if (!state) return;
  if (toolAsk?.tool_call_id === toolCallId) toolAsk = null;
  try {
    await invoke("llm_tool_answer", { requestId: state.requestId, toolCallId, allow, always });
  } catch {
    // Stub backend: nothing to answer.
  }
}

/** Test seam: drops every bit of state and every resource. */
export function resetLlmStore(): void {
  stopTurn();
  agents = [];
  rosterLoading = false;
  rosterAvailable = true;
  demoRoster = false;
  conversations = [];
  activeConversationId = null;
  toolAsk = null;
  errorKey = null;
  rosterLoadedOnce = false;
  rosterInFlight = null;
  idCounter = 0;
}
