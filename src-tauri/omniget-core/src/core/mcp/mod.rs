//! Own MCP client (stdio + Streamable HTTP), no SDK. Plan §2.1 line 10.
//! Owned by f3-mcp-core.
//!
//! The server we publish (`src-tauri/src/mcp.rs`) and the clients we consume
//! share [`types`], so a tool definition has one shape in both directions.
//!
//! Shape of the thing:
//!
//! ```text
//! McpRegistry ── config in <app_data>/llm/mcp-servers.json (no secrets)
//!      │  client_for(id)  → born on the first tools()/call(), reaped at 5 min idle
//!      ▼
//!   McpClient ── initialize → notifications/initialized → tools/list (cached)
//!      │
//!      ├── StdioTransport      child process, newline-delimited JSON-RPC
//!      └── HttpTransport       POST (JSON or text/event-stream), Mcp-Session-Id,
//!                              GET server stream, DELETE on close
//! ```
//!
//! Errors carry a stable `ERR_MCP_*` code (see [`error`]) and convert into the
//! `LlmError` the tool broker expects without losing it.

pub mod client;
pub mod error;
pub mod registry;
pub mod stdio;
pub mod streamable_http;
pub mod types;

pub use client::McpClient;
pub use error::{
    McpError, ERR_MCP_CANCELLED, ERR_MCP_HTTP, ERR_MCP_PROTO, ERR_MCP_SPAWN, ERR_MCP_TIMEOUT,
    ERR_MCP_TOOL,
};
pub use registry::{McpRegistry, IDLE_TIMEOUT};
pub use stdio::StdioTransport;
pub use streamable_http::HttpTransport;
pub use types::{
    McpServerConfig, ServerInfo, ToolDef, Transport, DEFAULT_TIMEOUT_MS, PROTOCOL_VERSION,
};
