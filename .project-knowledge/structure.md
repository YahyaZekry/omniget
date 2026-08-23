# Project Structure

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-16

## File Tree

```
omniget/
├── src/                      # SvelteKit frontend
│   ├── routes/               # +page.svelte routes: home, downloads, convert,
│   │                         #   courses, telegram, study (library/music/notes/
│   │                         #   reader/player/progress), league, marketplace,
│   │                         #   settings, about, misc
│   ├── lib/                  # Shared TS: stores/, i18n/, components/
│   │   ├── components/       # UI: buttons, dialogs, hints, omnibox, settings...
│   │   ├── home/             # Home screen logic
│   │   ├── stores/           # download-store, settings-store, media-preview, ...
│   │   ├── study-components/ # Study: notes/player/reader UI
│   │   ├── study-music/      # Music player logic
│   │   ├── study-notes/      # Notes app (tiptap, graph, mindmap, mermaid...)
│   │   └── i18n/             # Locale JSONs per language
│   ├── lib/reader-*.ts       # EPUB/PDF reader logic
│   └── app.css / theme       # Design tokens via CSS custom properties
├── src-tauri/                # Rust backend (Tauri 2)
│   ├── src/
│   │   ├── main.rs / lib.rs  # Entry points
│   │   ├── commands/         # Tauri IPC commands (ai, downloads, plugins,
│   │   │                     #   settings, search, p2p, channels, rules,
│   │   │                     #   bilibili_auth, auth_webview, subtitle_ws, ...)
│   │   ├── core/             # Engine: db (rusqlite), queue, hls_downloader,
│   │   │                     #   media_processor, events, channel_poller, cas
│   │   ├── platforms/        # Per-site download implementations (traits)
│   │   ├── models/           # media.rs, download.rs, settings.rs
│   │   ├── storage/          # config, database, cache
│   │   ├── plugin_loader.rs  # Loads Rust dylib plugins (ABI-locked to rustc)
│   │   ├── plugin_host.rs    # Plugin host API
│   │   ├── local_bridge.rs   # axum HTTP bridge on 127.0.0.1 (browser ext pairing)
│   │   ├── hotkey.rs         # Global Ctrl+Shift+D clipboard download
│   │   └── tray.rs           # System tray
│   ├── omniget-core/         # Shared download engine (workspace crate)
│   ├── omniget-plugin-sdk/   # Plugin SDK + build.rs ABI fingerprint
│   ├── omniget-cli/          # Scriptable CLI (info/download/batch/import-cookies)
│   └── tauri.conf.json       # Tauri config (createUpdaterArtifacts: false)
├── browser-extension/        # Chrome + Firefox extensions (share src/, sync.mjs)
├── flatpak/                  # Flatpak manifest + patches
├── docs/                     # plugin-development.md, backlog.md
├── scripts/                  # bump-version, i18n keys, clippy baseline, deploy-plugins
├── .github/workflows/        # ci.yml, release.yml
└── AGENTS.md                 # Design system + coding rules (authoritative)
```

## Key Files

| File | Purpose |
|------|---------|
| `src-tauri/src/commands/downloads.rs` | Core download IPC (queue, pause/resume/retry) |
| `src-tauri/src/core/db.rs` | SQLite schema + migrations |
| `src-tauri/src/plugin_loader.rs` | Runtime plugin loading (rustc-ABI fingerprint check) |
| `src-tauri/src/local_bridge.rs` | axum bridge for browser-extension pairing |
| `src-tauri/omniget-cli/src` | CLI entry |
| `src/routes/+page.svelte` | Home screen (omnibox, hotkey UX) |
| `src/lib/stores/download-store.ts` | Download queue state (frontend) |
| `src/lib/i18n/` | All UI strings via `$t()` |
| `src-tauri/desktop.template` | Handlebars desktop-entry template (`Categories=Network;FileTransfer;`) — used by deb, rpm, and AppImage bundling |
| `scripts/linux/linuxdeploy-plugin-gtk.sh` | Vendored patch of Tauri's GTK plugin: skips missing sources (Arch's stale gdk-pixbuf pkg-config paths), mkdir -p before writing loaders.cache. Synced into `~/.cache/tauri/` by `pnpm tauri:appimage` |
| `AGENTS.md` | Component patterns, tokens, a11y rules — read before UI work |
