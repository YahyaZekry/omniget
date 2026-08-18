# Features & Workflows

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-16

## Features

- **Universal downloader** — paste any link; site detection → preview with quality options → download. yt-dlp covers ~1,800+ sites; native extractors for major platforms. *(core)*
- **Course downloader** — full Udemy/Hotmart/Kiwify/Gumroad/Teachable/Kajabi/Skool/Wondrium/Thinkific courses incl. attached PDFs; plays inside app with resume + timestamped notes. *(courses plugin)*
- **Bilibili deep support** — 4K/HDR/Dolby, Hi-Res lossless, danmaku (XML/ASS/JSON), NFO for Kodi/Jellyfin, 11 URL types, signed-in 大会员 unlock. *(native, `commands/bilibili_auth.rs`)*
- **Bulk profiles** — whole subreddits + sort pages, Reddit/X user profiles, batch link lists from a text file. *(core engine)*
- **Audio extraction** — MP3/M4A/Opus/FLAC/WAV; start/end time clipping. *(core)*
- **Subtitles** — any language, embed, or Whisper-generate when none exist. *(`subtitle_ws.rs`, media_processor)*
- **SponsorBlock** — skips sponsors; auto-embeds metadata + thumbnails. *(media_processor)*
- **Telegram uploader/leech bot** — upload media via user session or bot token; Send-As modes; auto-chunking. *(telegram plugin)*
- **Dynamic YouTube quality picker** — real resolutions from metadata (2160p→audio-only). *(frontend quality picker)*
- **Bandwidth limiter** — throttle on the fly from downloads header. *(`commands/smart_speed.rs`)*
- **Channel following** — auto-download new uploads with tray notification. *(`core/channel_poller.rs`)*
- **Music library** — albums/covers, synced lyrics, Spotify/SoundCloud/YouTube Music/Qobuz/Last.fm connectors, EQ, Discord presence. *(`lib/study-music/`)*
- **Reader** — EPUB/PDF/CBZ/TXT/HTML with highlights, bookmarks, focus mode, paper theme. *(`lib/reader-*.ts`)*
- **Notes app** — bidirectional links, daily journal, knowledge graph, diagram views. *(`lib/study-notes/`)*
- **Subtitle Workshop** — timing tools, two-point sync, find/replace, auto-fix, AI translate. *(study components)*
- **Pomodoro focus timer** — pauses video at session end. *(study)*
- **FFmpeg converter** — local file conversion, offline. *(convert plugin)*
- **Portable mode** — `portable.txt` next to exe keeps everything in a local `data/` folder. *(app_lifecycle)*
- **System dependency detection** — uses system yt-dlp/FFmpeg/PDFium with source indicators (PATH/Managed/Flatpak). *(`commands/dependencies.rs`)*

## Workflows

**Clipboard → download (the one-keypress flow)**
1. User copies any URL — `hotkey.rs` global Ctrl/Cmd+Shift+D fires
2. `commands/clip.rs` reads clipboard → URL detection
3. Download enqueued → preview fetched (yt-dlp info) → format picked
4. Queue downloads, retries with backoff, resumes on restart
5. File lands in output folder; plays in-app

**Plugin lifecycle**
1. First launch reads `plugins/installed.json`; missing defaults auto-install (courses/telegram/convert)
2. Each plugin = Rust dylib built with the exact pinned rustc (ABI fingerprint check in `plugin_loader.rs`)
3. Update check against tonhowtf/omniget-plugins on launch
4. Toggle enable/disable/uninstall from sidebar

**Release (CI)**
1. Tag push → `release.yml` → matrix builds (macOS x2, ubuntu x2, windows)
2. tauri-action signs updater artifacts via `TAURI_CONFIG` override + secrets
3. zsync files, portable exe, CLI binaries, Chrome extension, `latest.json` all uploaded
