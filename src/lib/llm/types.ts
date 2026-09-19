/**
 * TypeScript mirror of the Rust LLM contract.
 *
 * Hand-written from `src-tauri/omniget-core/src/core/llm/types.rs` and
 * `.../llm/agent.rs` (no codegen). Serde attributes that matter here:
 *   - `ContentPart`, `TurnEvent`, `ToolSource`, `ModelPolicy`, `RuntimeKind`
 *     and `CandidateRuntime` are internally tagged; the tag is `type`,
 *     `source`, `policy`, `kind` or `runtime` as noted on each type.
 *   - every variant name is `snake_case` on the wire.
 * Keep this file in step with the Rust side; a mismatch is a silent bug.
 */

export type ProviderId = string;

export interface ModelRef {
  provider: ProviderId;
  model: string;
}

export type Role = "system" | "user" | "assistant" | "tool";

export type ContentPart =
  | { type: "text"; text: string }
  | { type: "image"; mime: string; data_b64: string }
  | { type: "tool_use"; id: string; name: string; input: unknown }
  | { type: "tool_result"; tool_use_id: string; content: string; is_error: boolean };

export interface Message {
  role: Role;
  parts: ContentPart[];
}

export interface ToolSpec {
  name: string;
  description: string;
  input_schema: unknown;
}

export interface GenParams {
  temperature?: number | null;
  top_p?: number | null;
  max_tokens?: number | null;
  reasoning_effort?: string | null;
  stop?: string[];
  extra?: Record<string, unknown>;
}

export interface Usage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
  first_token_ms?: number | null;
  total_ms: number;
  cost_usd?: number | null;
}

export type FinishReason =
  | "stop"
  | "length"
  | "tool_use"
  | "content_filter"
  | "cancelled"
  | "other";

/** Mirrors `LlmError` in `core/llm/error.rs`. */
export interface LlmError {
  code: string;
  message: string;
  retryable: boolean;
  retry_after_ms?: number | null;
}

/** Mirrors `TurnEvent` (tag `type`, snake_case). */
export type TurnEvent =
  | { type: "started"; request_id: string }
  | { type: "text_delta"; text: string }
  | { type: "thinking_delta"; text: string }
  | { type: "tool_call_start"; id: string; name: string }
  | { type: "tool_call_delta"; id: string; input_json_delta: string }
  | { type: "tool_call_end"; id: string }
  | { type: "usage"; usage: Usage }
  | { type: "tool_result"; id: string; content: string; is_error: boolean }
  | {
      type: "prune_receipt";
      request: number;
      omitted: number;
      est_tokens_before: number;
      est_tokens_after: number;
      input_tokens: number | null;
    }
  | { type: "finished"; reason: FinishReason }
  | { type: "error"; error: LlmError };

/** Payload of the `llm://turn` Tauri event. */
export interface TurnEventEnvelope {
  request_id: string;
  event: TurnEvent;
}

// ── Roster (core/llm/agent.rs) ──────────────────────────────────────────

/** `AgentRole` is externally tagged for `Custom`: `"worker"` or `{ custom: "…" }`. */
export type AgentRole =
  | "coordinator"
  | "worker"
  | "advisor"
  | { custom: string };

export type CandidateRuntime =
  | { runtime: "native"; provider: ProviderId }
  | { runtime: "cli"; account_id: string };

export type Candidate = CandidateRuntime & {
  model: string;
  max_cost_per_1k?: number | null;
  min_context?: number;
};

export type ModelPolicy =
  | { policy: "fixed"; model: ModelRef }
  | { policy: "route"; chain: Candidate[] };

export type ToolSource =
  | { source: "internal"; name: string }
  | { source: "mcp"; server: string; tool: string }
  | { source: "skill"; name: string };

export type GrantMode = "auto" | "ask" | "deny";

export type ToolGrant = ToolSource & { mode: GrantMode };

export interface Budget {
  usd_per_day?: number | null;
  tokens_per_turn?: number | null;
  max_tool_calls_per_turn?: number;
}

export type RuntimeKind =
  | { kind: "native" }
  | { kind: "cli"; cli: string; account: string }
  | { kind: "acp"; command: string; args?: string[] };

export interface Skin {
  id: string;
  /** RGB 0–255, same shape as the profile skin tint. */
  tint: [number, number, number];
}

export interface AgentDef {
  id: string;
  name: string;
  role: AgentRole;
  system_prompt: string;
  model: ModelPolicy;
  tools?: ToolGrant[];
  skills?: string[];
  budget?: Budget;
  runtime: RuntimeKind;
  skin?: Skin | null;
}

// ── UI-side shapes (not on the Rust wire) ───────────────────────────────

export interface ToolCallView {
  id: string;
  name: string;
  /** Concatenated `input_json_delta`; may be partial while streaming. */
  input: string;
  done: boolean;
}

export interface ChatMessage {
  id: string;
  role: Role;
  text: string;
  /** Set on assistant messages that ran tools. */
  toolCalls?: ToolCallView[];
  thinking?: string;
  usage?: Usage | null;
  /** `provider/model` at the time the message was produced. */
  modelLabel?: string;
  error?: LlmError | null;
  finish?: FinishReason | null;
}

export interface Conversation {
  id: string;
  agentId: string;
  title: string;
  messages: ChatMessage[];
  /** Overrides the agent's model for this conversation (`llm_switch_model`). */
  model?: ModelRef | null;
  updatedAtMs: number;
}

/** Payload of `llm://tool-ask` (re-emitted `BusEvent::ToolAsk`). */
export interface ToolAsk {
  agent: string;
  request_id: string;
  tool_call_id: string;
  tool: string;
  /** The command, the path or the head of the patch being asked about. */
  preview?: string;
}

export interface TurnState {
  requestId: string;
  conversationId: string;
  agentId: string;
  text: string;
  thinking: string;
  toolCalls: ToolCallView[];
  usage: Usage | null;
  error: LlmError | null;
  startedAtMs: number;
  /** True between `llm_turn_start` and the first `started` event. */
  starting: boolean;
}

// ── Helpers on the contract (pure) ──────────────────────────────────────

export function roleLabelKey(role: AgentRole): string {
  if (typeof role === "string") return `llm.role.${role}`;
  return "llm.role.custom";
}

export function roleText(role: AgentRole): string | null {
  return typeof role === "string" ? null : role.custom;
}

/** `provider/model` for the header and the inspector, or `""` when unknown. */
export function modelLabel(policy: ModelPolicy | null | undefined): string {
  if (!policy) return "";
  if (policy.policy === "fixed") return `${policy.model.provider}/${policy.model.model}`;
  const first = policy.chain[0];
  return first ? `${candidateProvider(first)}/${first.model}` : "";
}

export function candidateProvider(candidate: Candidate): string {
  return candidate.runtime === "native" ? candidate.provider : candidate.account_id;
}

export function modelRefLabel(ref: ModelRef | null | undefined): string {
  return ref ? `${ref.provider}/${ref.model}` : "";
}

/** Effective model of a conversation: the per-chat override wins. */
export function effectiveModelLabel(
  conversation: Conversation | null | undefined,
  agent: AgentDef | null | undefined,
): string {
  if (conversation?.model) return modelRefLabel(conversation.model);
  return modelLabel(agent?.model);
}

export function agentTint(agent: AgentDef | null | undefined): [number, number, number] {
  return agent?.skin?.tint ?? [110, 139, 255];
}
