#!/usr/bin/env node
// Cliente MCP de fora contra o servidor embutido do OmniGet (`POST /mcp` no
// bridge local, mesmo bearer da extensão). É o contrário do `mcp-fixture.mjs`,
// onde nós somos o cliente de um servidor de mentira.
//
// Uso (app aberto, Tools → AI → MCP server ligado, uma pasta anexada no /llm):
//   node scripts/mcp-client-smoke.mjs http://127.0.0.1:47720/mcp <token> [arquivo] [agente]
//
// Faz `initialize`, `tools/list`, `tools/call fs_read` na pasta ativa e, se um
// agente for passado, `tools/call agent_delegate`. Sai com 1 no primeiro erro.
//
// Rodado em 2026-09-19 contra o build de debug da 0.10.0 (perfil isolado, pasta
// ~/omniget-demo, agente claude-code):
//   node scripts/mcp-client-smoke.mjs http://127.0.0.1:47781/mcp $TOKEN src/cart.js claude-code
// Saída:
//   {"step":"initialize","server":"OmniGet","protocol":"2025-06-18"}
//   {"step":"tools/list","count":49,"has_fs_read":true,"has_agent_delegate":true}
//   {"step":"fs_read","isError":false,"lines":8,"path":"src/cart.js"}
//   {"step":"agent_delegate","isError":false,"agent":"claude-code","answer":"pong"}

const [url, token, file = "README.md", agent] = process.argv.slice(2);
if (!url || !token) {
  console.error("usage: mcp-client-smoke.mjs <http://127.0.0.1:PORT/mcp> <token> [file] [agent]");
  process.exit(2);
}

let id = 0;
let session = null;

async function rpc(method, params) {
  const headers = {
    "content-type": "application/json",
    accept: "application/json, text/event-stream",
    authorization: `Bearer ${token}`,
  };
  if (session) headers["mcp-session-id"] = session;
  const res = await fetch(url, {
    method: "POST",
    headers,
    body: JSON.stringify({ jsonrpc: "2.0", id: ++id, method, params }),
  });
  session = res.headers.get("mcp-session-id") ?? session;
  const text = await res.text();
  if (!res.ok) throw new Error(`${method}: HTTP ${res.status} ${text.slice(0, 200)}`);
  // Streamable HTTP may answer as one SSE event.
  const body = text.startsWith("event:") || text.startsWith("data:")
    ? text.split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5)).join("")
    : text;
  const msg = JSON.parse(body);
  if (msg.error) throw new Error(`${method}: ${JSON.stringify(msg.error)}`);
  return msg.result;
}

function payload(result) {
  if (result.structuredContent) return result.structuredContent;
  const text = result.content?.[0]?.text ?? "";
  try {
    return JSON.parse(text);
  } catch {
    return { text };
  }
}

try {
  const init = await rpc("initialize", {
    protocolVersion: "2025-06-18",
    capabilities: {},
    clientInfo: { name: "omniget-mcp-client-smoke", version: "1" },
  });
  console.log(JSON.stringify({ step: "initialize", server: init.serverInfo?.name, protocol: init.protocolVersion }));

  const list = await rpc("tools/list", {});
  const names = list.tools.map((t) => t.name);
  console.log(JSON.stringify({
    step: "tools/list",
    count: names.length,
    has_fs_read: names.includes("fs_read"),
    has_agent_delegate: names.includes("agent_delegate"),
  }));

  const read = await rpc("tools/call", { name: "fs_read", arguments: { path: file } });
  const r = payload(read);
  console.log(JSON.stringify({ step: "fs_read", isError: !!read.isError, lines: r.lines, path: r.path ?? r.text }));
  if (read.isError) process.exit(1);

  if (agent) {
    const del = await rpc("tools/call", {
      name: "agent_delegate",
      arguments: { agent_id: agent, task: "Reply with exactly one word: pong" },
    });
    const d = payload(del);
    console.log(JSON.stringify({ step: "agent_delegate", isError: !!del.isError, agent: d.agent, answer: d.answer ?? d.text }));
    if (del.isError) process.exit(1);
  }
} catch (e) {
  console.error(String(e));
  process.exit(1);
}
