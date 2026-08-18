# Systems

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-16

| System | Status | Details |
|--------|--------|---------|
| Download Engine | Active | yt-dlp driven; per-site platform impls via traits; queue with retry/backoff/resume; explicit phases (Preparing → Fetching Info → Starting → Connecting → Downloading); dynamic quality picker from real metadata; explicit merge phase |
| Plugin System | Active | Rust dynamic libraries (`.so`) loaded at runtime; host + plugin must share exact rustc (pinned 1.97.0); default plugins: courses, telegram, convert; registry at tonhowtf/omniget-plugins |
| Updater | Active | tauri-plugin-updater; `latest.json` + `.sig` on GitHub releases; signing only on tagged releases (`createUpdaterArtifacts` false by default since PR #290) |
| Local Bridge | Active | axum HTTP server on `127.0.0.1` (ephemeral port) — used by browser-extension pairing & local IPC fallback |
| Global Hotkey | Active | Ctrl/Cmd+Shift+D reads clipboard and enqueues download |
| Clipboard Monitor | Active | Detects video URLs on clipboard, shows single-click toast banner |
| Cookies/Auth | Active | Platform session cookies (`cookies/` bucket store); `auth_webview` embedded login for sites; Bilibili 大会员 unlock |
| P2P Transfer | Active | Direct transfer between two computers via short code |
| Torrents | Active | `.torrent`/magnet downloads via engine |
| Telegram | Plugin | Uploader/leech bot, chat browser, save media from chats (2GB/4GB chunking) |
| League of Legends | Active (opt-in) | Match scouting, win probability, live economy; off by default |
| Notes/Study | Active | tiptap notes, bidirectional links, knowledge graph, mindmap/mermaid/plantuml, spaced repetition (anki-bridge) |
| Music Library | Active | Local + Spotify/SoundCloud/YMusic/Qobuz/Last.fm, synced lyrics, EQ, Discord presence |
| Reader | Active | EPUB/PDF/CBZ/TXT/HTML, highlights, bookmarks, focus mode |
| Subtitle Workshop | Active | SRT/VTT/ASS editing, two-point sync, AI translate/grammar, waveform |
| Tracker / Dashboard | Active | Streaks, daily goals, heatmap, gamification |
| Browser Extension | Active | Chrome + Firefox; one click hands current page to OmniGet |
