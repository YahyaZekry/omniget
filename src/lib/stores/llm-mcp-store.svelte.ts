/**
 * State of `/llm/mcp`: the external MCP servers, what each one exposes and the
 * per-agent grants.
 *
 * Three rules, same shape as `llm-store.svelte.ts`:
 *
 * 1. **Nothing runs at rest.** No listener, no timer, no polling. Reading the
 *    saved list is a file read; a *connection* only happens when the user
 *    presses "test" or opens a server's tool table.
 * 2. **`tools/list` is cached.** Once a server answered, its tools stay in the
 *    store until the user tests again, so opening the grant table twice costs
 *    one connection, not two.
 * 3. **A missing backend is not a failure.** `ERR_STUB` (a command that is not
 *    wired) and `null` (what the screenshot harness answers for an unknown
 *    command) both put the store in demo mode: a pair of example servers, so
 *    the tab is reviewable without a backend. `isDemo()` drives the banner
 *    that says as much, and nothing is presented as real.
 */
import { invoke } from "@tauri-apps/api/core";
import type { AgentDef, GrantMode } from "$lib/llm/types";
import { saveAgent } from "$lib/stores/llm-store.svelte";
import {
  applyGrant,
  mcpErrorKey,
  type McpServerConfig,
  type McpServerRow,
  type McpTestResult,
  type McpToolDef,
} from "$lib/llm/mcp";

// ── Demo data (shown only when the backend is not wired) ────────────────

function demoRows(): McpServerRow[] {
  return [
    {
      config: {
        id: "filesystem",
        name: "Filesystem",
        enabled: true,
        transport: {
          kind: "stdio",
          command: "npx",
          args: ["-y", "@modelcontextprotocol/server-filesystem", "/Users/demo/Notes"],
          env: {},
          cwd: null,
        },
      },
      connected: false,
      last_error: null,
      tools: [
        {
          name: "read_file",
          description: "Read the complete contents of a file.",
          inputSchema: { properties: { path: {} }, required: ["path"] },
        },
        {
          name: "write_file",
          description: "Create a new file or overwrite an existing one.",
          inputSchema: { properties: { path: {}, content: {} }, required: ["path", "content"] },
        },
        {
          name: "list_directory",
          description: "List the files and folders of a directory.",
          inputSchema: { properties: { path: {} }, required: ["path"] },
        },
      ],
    },
    {
      config: {
        id: "context7",
        name: "Context7",
        enabled: false,
        transport: { kind: "http", url: "https://mcp.context7.com/mcp", headers: {} },
      },
      connected: false,
      last_error: null,
      tools: [],
    },
  ];
}

// ── State ───────────────────────────────────────────────────────────────

let rows = $state<McpServerRow[]>([]);
let loading = $state(false);
let demo = $state(false);
let available = $state(true);
let errorKey = $state<string | null>(null);
let testing = $state<string | null>(null);
let results = $state<Record<string, McpTestResult>>({});

let loadedOnce = false;
let inFlight: Promise<void> | null = null;

export function getServers(): McpServerRow[] {
  return rows;
}

export function getServer(id: string | null | undefined): McpServerRow | null {
  if (!id) return null;
  return rows.find((r) => r.config.id === id) ?? null;
}

export function isLoading(): boolean {
  return loading;
}

/** True while the tab is showing the demo pair instead of saved servers. */
export function isDemo(): boolean {
  return demo;
}

/** False once a command answered something other than `ERR_STUB`. */
export function isAvailable(): boolean {
  return available;
}

export function getErrorKey(): string | null {
  return errorKey;
}

/** Id of the server being probed right now, or `null`. One at a time. */
export function getTestingId(): string | null {
  return testing;
}

export function getTestResult(id: string): McpTestResult | null {
  return results[id] ?? null;
}

export function getTools(id: string): McpToolDef[] {
  return getServer(id)?.tools ?? [];
}

function isStub(err: unknown): boolean {
  return String(err ?? "").includes("ERR_STUB");
}

/** Normalises both shapes the backend may answer: a list or `{ servers }`. */
function toRows(answer: unknown): McpServerRow[] | null {
  if (Array.isArray(answer)) return answer as McpServerRow[];
  if (answer && typeof answer === "object" && Array.isArray((answer as { servers?: unknown }).servers)) {
    return (answer as { servers: McpServerRow[] }).servers;
  }
  return null;
}

/**
 * Reads the saved servers. No connection is opened: this is the config file,
 * not the network. Loads once per session unless `force`. Never throws.
 */
export function loadServers(force = false): Promise<void> {
  if (inFlight) return inFlight;
  if (loadedOnce && !force) return Promise.resolve();
  loading = true;
  inFlight = invoke<unknown>("llm_mcp_list")
    .then((answer) => {
      const list = toRows(answer);
      if (list) {
        rows = list;
        demo = false;
        available = true;
        return;
      }
      // `null`: the screenshot harness, or a backend that does not know the
      // command yet.
      rows = demoRows();
      demo = true;
      available = false;
    })
    .catch((err) => {
      rows = demoRows();
      demo = true;
      available = false;
      // `ERR_STUB` is the backend saying "not wired yet", which the banner
      // already explains; anything else is a failure worth naming.
      if (!isStub(err)) errorKey = mcpErrorKey(err);
    })
    .finally(() => {
      loading = false;
      loadedOnce = true;
      inFlight = null;
    });
  return inFlight;
}

/** Creates or replaces one server. Returns false when the backend refused. */
export async function saveServer(config: McpServerConfig): Promise<boolean> {
  errorKey = null;
  const existing = rows.some((r) => r.config.id === config.id);
  rows = existing
    ? rows.map((r) => (r.config.id === config.id ? { ...r, config } : r))
    : [...rows, { config, tools: [], connected: false, last_error: null }];
  try {
    const answer = await invoke<unknown>("llm_mcp_upsert", { config });
    const list = toRows(answer);
    if (list) {
      rows = list;
      demo = false;
      available = true;
    }
    return true;
  } catch (err) {
    if (!isStub(err)) {
      available = false;
      errorKey = mcpErrorKey(err);
      return false;
    }
    return true; // stub backend: the optimistic row stands in
  }
}

export async function removeServer(id: string): Promise<boolean> {
  errorKey = null;
  rows = rows.filter((r) => r.config.id !== id);
  delete results[id];
  try {
    const answer = await invoke<unknown>("llm_mcp_remove", { id });
    const list = toRows(answer);
    if (list) rows = list;
    return true;
  } catch (err) {
    if (!isStub(err)) {
      available = false;
      errorKey = mcpErrorKey(err);
      return false;
    }
    return true;
  }
}

/**
 * The one command that opens a connection: `initialize` + `tools/list`, then
 * the client is dropped. Only ever called from the "test" button.
 */
export async function testServer(id: string): Promise<McpTestResult | null> {
  if (testing) return null;
  errorKey = null;
  testing = id;
  const startedAt = Date.now();
  try {
    const result = await invoke<McpTestResult | null>("llm_mcp_test", { id });
    if (!result) {
      // Harness or unknown command: keep whatever the row already showed.
      return null;
    }
    results = { ...results, [id]: result };
    const row = rows.find((r) => r.config.id === id);
    if (row) {
      row.connected = result.ok;
      row.last_error = result.error ?? null;
    }
    if (result.ok) await loadTools(id, true);
    return result;
  } catch (err) {
    const result: McpTestResult = {
      ok: false,
      tool_count: 0,
      elapsed_ms: Date.now() - startedAt,
      error: { code: String(err ?? ""), message: String(err ?? "") },
    };
    if (!isStub(err)) {
      results = { ...results, [id]: result };
      const row = rows.find((r) => r.config.id === id);
      if (row) {
        row.connected = false;
        row.last_error = result.error ?? null;
      }
      errorKey = mcpErrorKey(err);
      available = false;
      return result;
    }
    return null;
  } finally {
    testing = null;
  }
}

/**
 * Tools of one server. Cached: with tools already in the row nothing is sent
 * unless `force`. Called when the grant table opens, never on page load.
 */
export async function loadTools(id: string, force = false): Promise<McpToolDef[]> {
  const row = rows.find((r) => r.config.id === id);
  if (!row) return [];
  if (row.tools.length > 0 && !force) return row.tools;
  try {
    const answer = await invoke<McpToolDef[] | null>("llm_mcp_tools", { id });
    if (Array.isArray(answer)) {
      row.tools = answer;
      row.connected = true;
      row.last_error = null;
    }
    return row.tools;
  } catch (err) {
    if (!isStub(err)) {
      row.last_error = { code: String(err ?? ""), message: String(err ?? "") };
      errorKey = mcpErrorKey(err);
    }
    return row.tools;
  }
}

/**
 * Writes one grant into `AgentDef.tools`; `null` clears it.
 *
 * `llm_mcp_grant` is the narrow command — it touches one grant and leaves the
 * rest of the agent alone. When it is not there yet the whole agent is saved
 * through the roster command that already exists, from the same pure
 * `applyGrant`, so the tab works either way and never two writes for one
 * click.
 */
export async function setGrant(
  agent: AgentDef,
  server: string,
  tool: string,
  mode: GrantMode | null,
): Promise<boolean> {
  errorKey = null;
  const next = applyGrant(agent.tools, server, tool, mode);
  agent.tools = next;
  try {
    await invoke("llm_mcp_grant", {
      agentId: agent.id,
      grant: { source: "mcp", server, tool, mode },
    });
    return true;
  } catch (err) {
    if (!isStub(err)) {
      errorKey = mcpErrorKey(err);
      return false;
    }
  }
  return await saveAgent({ ...agent, tools: next });
}

/** Test seam: drops every bit of state. Nothing to cancel — nothing runs. */
export function resetMcpStore(): void {
  rows = [];
  loading = false;
  demo = false;
  available = true;
  errorKey = null;
  testing = null;
  results = {};
  loadedOnce = false;
  inFlight = null;
}
