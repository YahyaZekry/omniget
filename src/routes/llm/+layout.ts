// The LLM section is client-only: every command and event needs the Tauri
// bridge, so there is nothing to render on the server.
export const ssr = false;
export const prerender = false;
