import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { McpServerRow } from "$lib/llm/mcp";
import type { AgentDef } from "$lib/llm/types";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

type McpStore = typeof import("./llm-mcp-store.svelte");

let store: McpStore;

beforeAll(async () => {
  vi.stubGlobal("$state", <T>(value: T) => value);
  vi.stubGlobal("$derived", <T>(value: T) => value);
  store = await import("./llm-mcp-store.svelte");
});

afterEach(() => {
  store.resetMcpStore();
  invoke.mockReset();
});

afterAll(() => {
  vi.unstubAllGlobals();
});

function agent(): AgentDef {
  return {
    id: "omni",
    name: "Omni",
    role: "worker",
    system_prompt: "",
    model: { policy: "fixed", model: { provider: "openai", model: "gpt-5" } },
    tools: [],
    skills: [],
    budget: {},
    runtime: { kind: "native" },
  };
}

const ROW: McpServerRow = {
  config: {
    id: "fetch",
    name: "Fetch",
    enabled: true,
    transport: { kind: "stdio", command: "uvx", args: ["mcp-server-fetch"], env: {}, cwd: null },
  },
  tools: [],
  connected: false,
  last_error: null,
};

describe("loadServers", () => {
  it("shows the saved servers when the backend answers", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    expect(store.getServers()).toHaveLength(1);
    expect(store.isDemo()).toBe(false);
    expect(store.isAvailable()).toBe(true);
    expect(invoke).toHaveBeenCalledWith("llm_mcp_list");
  });

  it("falls back to demo servers on ERR_STUB, without calling it an error", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    await store.loadServers();
    expect(store.getServers().length).toBeGreaterThan(0);
    expect(store.isDemo()).toBe(true);
    expect(store.isAvailable()).toBe(false);
    expect(store.getErrorKey()).toBeNull();
  });

  it("falls back to demo servers when the harness answers null", async () => {
    invoke.mockResolvedValue(null);
    await store.loadServers();
    expect(store.isDemo()).toBe(true);
    expect(store.isAvailable()).toBe(false);
  });

  it("reports a real failure with an i18n key", async () => {
    invoke.mockRejectedValue("ERR_MCP_HTTP: 503");
    await store.loadServers();
    expect(store.isAvailable()).toBe(false);
    expect(store.getErrorKey()).toBe("llm.mcp.err.http");
  });

  it("loads once per session and only reloads when forced", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    await store.loadServers();
    expect(invoke).toHaveBeenCalledTimes(1);
    await store.loadServers(true);
    expect(invoke).toHaveBeenCalledTimes(2);
  });

  it("collapses concurrent loads into one call", async () => {
    invoke.mockResolvedValue([ROW]);
    await Promise.all([store.loadServers(), store.loadServers(), store.loadServers()]);
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it("accepts the { servers } shape too", async () => {
    invoke.mockResolvedValue({ servers: [ROW] });
    await store.loadServers();
    expect(store.getServers()).toHaveLength(1);
    expect(store.isDemo()).toBe(false);
  });
});

describe("saveServer / removeServer", () => {
  it("sends the config and takes the list the backend answers", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    invoke.mockResolvedValue([ROW, { ...ROW, config: { ...ROW.config, id: "other" } }]);
    expect(await store.saveServer(ROW.config)).toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("llm_mcp_upsert", { config: ROW.config });
    expect(store.getServers()).toHaveLength(2);
  });

  it("keeps the optimistic row when the backend is a stub", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    await store.loadServers();
    const before = store.getServers().length;
    expect(await store.saveServer(ROW.config)).toBe(true);
    expect(store.getServers()).toHaveLength(before + 1);
  });

  it("surfaces a refused config", async () => {
    invoke.mockResolvedValue([]);
    await store.loadServers();
    invoke.mockRejectedValue("ERR_MCP_CONFIG: url");
    expect(await store.saveServer(ROW.config)).toBe(false);
    expect(store.getErrorKey()).toBe("llm.mcp.err.config");
  });

  it("removes a server by id", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    invoke.mockResolvedValue([]);
    expect(await store.removeServer("fetch")).toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("llm_mcp_remove", { id: "fetch" });
    expect(store.getServers()).toEqual([]);
  });
});

describe("testServer", () => {
  it("is the only path that connects, and caches the tools it found", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    invoke.mockReset();
    invoke.mockImplementation((cmd: string) => {
      if (cmd === "llm_mcp_test") {
        return Promise.resolve({ ok: true, tool_count: 1, elapsed_ms: 42, error: null });
      }
      if (cmd === "llm_mcp_tools") {
        return Promise.resolve([{ name: "fetch", description: "Fetch a url" }]);
      }
      return Promise.reject("unexpected");
    });
    const result = await store.testServer("fetch");
    expect(result?.ok).toBe(true);
    expect(store.getTestResult("fetch")?.elapsed_ms).toBe(42);
    expect(store.getTools("fetch")).toHaveLength(1);
    expect(store.getServer("fetch")?.connected).toBe(true);

    // The grant table opening again must not connect a second time.
    invoke.mockClear();
    await store.loadTools("fetch");
    expect(invoke).not.toHaveBeenCalled();
  });

  it("records the failure on the row instead of throwing", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    invoke.mockRejectedValue("ERR_MCP_SPAWN: uvx not found");
    const result = await store.testServer("fetch");
    expect(result?.ok).toBe(false);
    expect(store.getServer("fetch")?.connected).toBe(false);
    expect(store.getServer("fetch")?.last_error?.code).toContain("ERR_MCP_SPAWN");
    expect(store.getErrorKey()).toBe("llm.mcp.err.spawn");
  });

  it("refuses a second probe while one is in flight", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    let release: (value: unknown) => void = () => {};
    invoke.mockImplementation(() => new Promise((resolve) => (release = resolve)));
    const first = store.testServer("fetch");
    expect(store.getTestingId()).toBe("fetch");
    expect(await store.testServer("fetch")).toBeNull();
    release({ ok: false, tool_count: 0, elapsed_ms: 1, error: null });
    await first;
    expect(store.getTestingId()).toBeNull();
  });

  it("stays quiet on ERR_STUB", async () => {
    invoke.mockRejectedValue("ERR_STUB");
    await store.loadServers();
    expect(await store.testServer("filesystem")).toBeNull();
    expect(store.getErrorKey()).toBeNull();
  });
});

describe("setGrant", () => {
  it("uses llm_mcp_grant when it exists", async () => {
    invoke.mockResolvedValue([]);
    const a = agent();
    expect(await store.setGrant(a, "fetch", "fetch", "ask")).toBe(true);
    expect(invoke).toHaveBeenCalledWith("llm_mcp_grant", {
      agentId: "omni",
      grant: { source: "mcp", server: "fetch", tool: "fetch", mode: "ask" },
    });
    expect(a.tools).toEqual([{ source: "mcp", server: "fetch", tool: "fetch", mode: "ask" }]);
  });

  it("falls back to the roster command while llm_mcp_grant is a stub", async () => {
    invoke.mockImplementation((cmd: string) =>
      cmd === "llm_mcp_grant" ? Promise.reject("ERR_STUB") : Promise.resolve(null),
    );
    const a = agent();
    expect(await store.setGrant(a, "fetch", "fetch", "auto")).toBe(true);
    const calls = invoke.mock.calls.map((c) => c[0]);
    expect(calls).toContain("llm_roster_create");
  });

  it("clears a grant with a null mode", async () => {
    invoke.mockResolvedValue([]);
    const a = agent();
    a.tools = [{ source: "mcp", server: "fetch", tool: "fetch", mode: "auto" }];
    await store.setGrant(a, "fetch", "fetch", null);
    expect(a.tools).toEqual([]);
    expect(invoke).toHaveBeenCalledWith("llm_mcp_grant", {
      agentId: "omni",
      grant: { source: "mcp", server: "fetch", tool: "fetch", mode: null },
    });
  });
});

describe("the budget", () => {
  it("opens no connection when the tab loads", async () => {
    invoke.mockResolvedValue([ROW]);
    await store.loadServers();
    const commands = invoke.mock.calls.map((c) => c[0]);
    expect(commands).toEqual(["llm_mcp_list"]);
    expect(commands).not.toContain("llm_mcp_test");
    expect(commands).not.toContain("llm_mcp_tools");
  });

  it("leaves nothing running after a reset", () => {
    store.resetMcpStore();
    expect(store.getServers()).toEqual([]);
    expect(store.getTestingId()).toBeNull();
    expect(store.isLoading()).toBe(false);
  });
});
