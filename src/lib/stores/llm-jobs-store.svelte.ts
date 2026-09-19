/**
 * Agent jobs, loops and triggers. One load on first use, then the backend
 * pushes `llm://job` and `llm://loop`; rows are upserted by id.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { showToast } from "$lib/stores/toast-store.svelte";

export type JobState = "queued" | "running" | "waiting_approval" | "done" | "failed" | "cancelled";

export type Job = {
  id: string;
  kind: "run" | "chat" | "loop" | "trigger";
  agent_id: string;
  conversation_id: string;
  prompt: string;
  workspace: string | null;
  state: JobState;
  loop_id: string | null;
  trigger_id: string | null;
  request_id: string | null;
  created_ms: number;
  started_ms: number | null;
  finished_ms: number | null;
  result: string | null;
  error: string | null;
  log: string;
  /** Summed over the model calls of the job; `cost_usd` is null for local models and CLI accounts. */
  usage?: {
    model: string;
    input_tokens: number;
    output_tokens: number;
    cache_read_tokens: number;
    cost_usd: number | null;
    calls: number;
    /** Context pruning, per model request of the job. Measured, never a ratio. */
    prune?: {
      omitted: number;
      requests: {
        request: number;
        omitted: number;
        est_tokens_before: number;
        est_tokens_after: number;
        input_tokens: number | null;
      }[];
    };
  } | null;
};

/** `ollama/qwen3:8b · 8.2k in · 1.7k out · $0.0123` for a job row. */
export function usageLabel(job: Job): string {
  const u = job.usage;
  if (!u) return "";
  const k = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
  const parts = [u.model, `${k(u.input_tokens)} in`, `${k(u.output_tokens)} out`];
  if (u.cost_usd != null) parts.push(`$${u.cost_usd.toFixed(4)}`);
  const last = u.prune?.requests.at(-1);
  if (last && last.omitted > 0) {
    parts.push(`${last.omitted} omitted, est. ${k(last.est_tokens_before)} → ${k(last.est_tokens_after)}`);
  }
  return parts.filter(Boolean).join(" · ");
}

export type LoopDef = {
  id: string;
  name: string;
  agent_id: string;
  prompt: string;
  workspace: string | null;
  max_rounds: number | null;
  max_minutes: number | null;
  check_command: string | null;
  state: "running" | "done" | "failed" | "cancelled";
  rounds_done: number;
  conversation_id: string;
  created_ms: number;
  finished_ms: number | null;
  last_check: string | null;
  stop_reason: string | null;
};

export type Trigger = {
  id: string;
  name: string;
  kind: "cron" | "webhook";
  cron: string | null;
  agent_id: string;
  prompt: string;
  workspace: string | null;
  enabled: boolean;
  last_fired_ms: number | null;
  fire_count: number;
  created_ms: number;
};

export type BridgeInfo = { base_url: string; token: string };

export type PendingAsk = {
  agent: string;
  request_id: string;
  tool_call_id: string;
  tool: string;
  preview: string;
};

export type LoopInput = {
  agent_id: string;
  prompt: string;
  name?: string;
  workspace?: string | null;
  max_rounds?: number | null;
  max_minutes?: number | null;
  check_command?: string | null;
};

export type TriggerInput = {
  id?: string;
  name?: string;
  kind: "cron" | "webhook";
  cron?: string | null;
  agent_id: string;
  prompt: string;
  workspace?: string | null;
  enabled: boolean;
};

let jobs = $state<Job[]>([]);
let loops = $state<LoopDef[]>([]);
let triggers = $state<Trigger[]>([]);
let bridge = $state<BridgeInfo | null>(null);
let asks = $state<PendingAsk[]>([]);
/** Last `llm://job` payload, so an open row can refresh its log. */
let lastJobEvent = $state<Job | null>(null);
let started = false;

export function getJobs(): Job[] {
  return jobs;
}
export function getLoops(): LoopDef[] {
  return loops;
}
export function getTriggers(): Trigger[] {
  return triggers;
}
export function getBridge(): BridgeInfo | null {
  return bridge;
}
export function getAsks(): PendingAsk[] {
  return asks;
}
export function getLastJobEvent(): Job | null {
  return lastJobEvent;
}

function fail(e: unknown) {
  showToast("error", String(e));
}

function upsert<T extends { id: string }>(list: T[], row: T): T[] {
  const i = list.findIndex((x) => x.id === row.id);
  if (i < 0) return [row, ...list];
  const next = list.slice();
  next[i] = row;
  return next;
}

export async function reloadJobs(): Promise<void> {
  try {
    jobs = await invoke<Job[]>("llm_jobs_list", { limit: 200 });
  } catch (e) {
    fail(e);
  }
}

export async function reloadLoops(): Promise<void> {
  try {
    loops = await invoke<LoopDef[]>("llm_loops_list");
  } catch (e) {
    fail(e);
  }
}

export async function reloadTriggers(): Promise<void> {
  try {
    const out = await invoke<{ triggers: Trigger[]; bridge: BridgeInfo }>("llm_triggers_list");
    triggers = out.triggers ?? [];
    bridge = out.bridge ?? null;
  } catch (e) {
    fail(e);
  }
}

export function initJobsStore(): void {
  if (started) return;
  started = true;
  void reloadJobs();
  void reloadLoops();
  void reloadTriggers();
  void listen<Job>("llm://job", (ev) => {
    jobs = upsert(jobs, ev.payload);
    lastJobEvent = ev.payload;
  }).catch(() => {});
  void listen<LoopDef>("llm://loop", (ev) => {
    loops = upsert(loops, ev.payload);
  }).catch(() => {});
}

export async function fetchJob(id: string): Promise<Job | null> {
  try {
    return await invoke<Job | null>("llm_job_get", { id });
  } catch (e) {
    fail(e);
    return null;
  }
}

export async function submitJob(agentId: string, prompt: string, workspace: string | null): Promise<Job | null> {
  try {
    const job = await invoke<Job>("llm_job_submit", { agentId, prompt, workspace });
    jobs = upsert(jobs, job);
    return job;
  } catch (e) {
    fail(e);
    return null;
  }
}

export async function cancelJob(id: string): Promise<void> {
  try {
    jobs = upsert(jobs, await invoke<Job>("llm_job_cancel", { id }));
  } catch (e) {
    fail(e);
  }
}

export async function deleteJob(id: string): Promise<void> {
  try {
    await invoke("llm_job_delete", { id });
    jobs = jobs.filter((j) => j.id !== id);
  } catch (e) {
    fail(e);
  }
}

export async function createLoop(def: LoopInput): Promise<LoopDef | null> {
  try {
    const loop = await invoke<LoopDef>("llm_loop_create", { def });
    loops = upsert(loops, loop);
    return loop;
  } catch (e) {
    fail(e);
    return null;
  }
}

export async function cancelLoop(id: string): Promise<void> {
  try {
    loops = upsert(loops, await invoke<LoopDef>("llm_loop_cancel", { id }));
  } catch (e) {
    fail(e);
  }
}

export async function deleteLoop(id: string): Promise<void> {
  try {
    await invoke("llm_loop_delete", { id });
    loops = loops.filter((l) => l.id !== id);
  } catch (e) {
    fail(e);
  }
}

export async function saveTrigger(trigger: TriggerInput): Promise<Trigger | null> {
  try {
    const saved = await invoke<Trigger>("llm_trigger_save", { trigger });
    triggers = upsert(triggers, saved);
    return saved;
  } catch (e) {
    fail(e);
    return null;
  }
}

export async function deleteTrigger(id: string): Promise<void> {
  try {
    await invoke("llm_trigger_delete", { id });
    triggers = triggers.filter((x) => x.id !== id);
  } catch (e) {
    fail(e);
  }
}

export async function fireTrigger(id: string): Promise<Job | null> {
  try {
    const job = await invoke<Job>("llm_trigger_fire", { id });
    jobs = upsert(jobs, job);
    return job;
  } catch (e) {
    fail(e);
    return null;
  }
}

export async function pollAsks(): Promise<void> {
  try {
    asks = await invoke<PendingAsk[]>("llm_tool_asks_pending");
  } catch {
    asks = [];
  }
}

export async function answerAsk(ask: PendingAsk, allow: boolean, always: boolean): Promise<void> {
  try {
    await invoke("llm_tool_answer", {
      requestId: ask.request_id,
      toolCallId: ask.tool_call_id,
      allow,
      always,
    });
    asks = asks.filter((a) => a.tool_call_id !== ask.tool_call_id);
  } catch (e) {
    fail(e);
  }
}

/** Shared by both pages. */
export function pickState(state: string): string {
  switch (state) {
    case "running":
      return "blue";
    case "waiting_approval":
      return "orange";
    case "done":
      return "green";
    case "failed":
      return "red";
    default:
      return "grey";
  }
}

export function shortTime(ms: number | null): string {
  if (!ms) return "";
  const d = new Date(ms);
  const sameDay = new Date().toDateString() === d.toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleString([], { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

export function duration(fromMs: number | null, toMs: number | null): string {
  if (!fromMs || !toMs || toMs < fromMs) return "";
  const s = Math.round((toMs - fromMs) / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

export async function pickFolder(): Promise<string | null> {
  try {
    const { open } = await import("@tauri-apps/plugin-dialog");
    const out = await open({ directory: true, multiple: false });
    return typeof out === "string" ? out : null;
  } catch (e) {
    fail(e);
    return null;
  }
}
