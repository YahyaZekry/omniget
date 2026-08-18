# Stack

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-16

## Tech Stack

| Category | Details |
|----------|---------|
| Language | TypeScript (strict, frontend) + Rust (backend) |
| Runtime | Node.js 18+; WebView (WebKitGTK on Linux) |
| Framework | SvelteKit 2 / Svelte 5 runes; Tauri 2 (Rust) |
| Database | SQLite via `rusqlite` (bundled) |
| ORM / Query | None — raw `rusqlite` |
| Auth | Platform sessions/cookies only (`auth_webview`, `bilibili_auth`) |
| Styling | Scoped CSS + custom properties; NO Tailwind |
| State Mgmt | Svelte stores in `$lib/stores/` (partial-merge settings store) |
| Testing | vitest (frontend); `scripts/clippy-baseline.mjs` (Rust) |
| Key Libraries | yt-dlp + FFmpeg (external binaries, bundled), `@tabler/icons-svelte`, tiptap, cytoscape, m3u8-rs, reqwest, axum (local bridge) |
| Dev Tools | pnpm 10, cargo (rustup, pinned 1.97.0 via `rust-toolchain.toml`), tauri-cli |
| Deployment | GitHub Actions (`release.yml`), tauri-action, deb/rpm/AppImage bundles, flatpak manifest |

Rust workspace: root `src-tauri` + `omniget-core` (shared engine) + `omniget-plugin-sdk` (plugin ABI) + `omniget-cli` (scriptable CLI).

---

## Dev Commands

| Command | What It Does |
|---------|-------------|
| `pnpm install` | Install frontend deps (pnpm 10; esbuild/@swc-core postinstall approved via `pnpm.onlyBuiltDependencies`) |
| `pnpm dev` | SvelteKit dev server only |
| `pnpm tauri dev` | Full app (Rust + frontend) — same as `cargo tauri dev` |
| `pnpm build` | Production frontend build → `build/` |
| `pnpm tauri build --bundles deb` | Release build (Linux deb; no signing key needed) |
| `pnpm tauri build` | Release build, all bundle targets |
| `pnpm check` | `svelte-kit sync && svelte-check --tsconfig ./tsconfig.json` |
| `pnpm test` | vitest run |
| `cargo check` | Typecheck Rust without building |
| `cargo build -p omniget-cli --release` | Build the scriptable CLI |

---

## Environment Variables

| Variable | Used In | What It Enables |
|----------|---------|----------------|
| `TAURI_SIGNING_PRIVATE_KEY` | release.yml (tauri-action) | Sign updater artifacts (only needed for tagged releases) |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | release.yml (tauri-action) | Password for the signing key |
| `TAURI_CONFIG` | release.yml | Deep-merge config override — used to re-enable `bundle.createUpdaterArtifacts` on releases |
| `LDAI_UPDATE_INFORMATION` | release.yml | AppImage zsync update info |
| `NODE_OPTIONS` | release.yml | `--max-old-space-size=6144` for CI memory |
