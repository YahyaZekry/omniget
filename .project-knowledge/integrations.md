# External Integrations & Data Contracts

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-16
> Document exact contracts — never guess the shape.

## yt-dlp (download engine)

- Writes to: downloaded media files (paths in SQLite `downloads` table)
- Reads from: platform pages/APIs via extractors (~1,800+ sites)
- Contract: spawned as subprocess; parses stdout progress lines (`[download]`, `[Merger]`, `[ffmpeg]`) into progress/phase events
- Version: bundled binary, self-updating; verified by SHA256 before run; system-installed one detectable too (`commands/dependencies.rs`)

## FFmpeg

- Used for: merge (Merger phase), audio extraction, re-encode, subtitle embed, SponsorBlock cut
- Bundled/self-installing; system binary detectable with source indicator

## Platform cookies / auth

- Hotmart, Udemy, Kiwify, Skool, Teachable, Kajabi, Wondrium, Thinkific, Bilibili (大会员), etc.
- Stored in cookie buckets (`storage/`, `commands/cookies/`); sessions stay local; downloads only what the logged-in session can reach
- `auth_webview.rs` opens embedded login webview to capture sessions

## Plugin registry — tonhowtf/omniget-plugins

- Install/update via GitHub; default set: courses, telegram, convert
- Contract: Rust dylib with SDK from `omniget-plugin-sdk`; ABI locked to rustc 1.97.0 (see `plugin-development.md`)

## Updater

- GitHub Releases `latest.json` + `.sig` per platform bundle; tauri-plugin-updater; pubkey `wtf.tonho.omniget`
- Contract: `{version, notes, pub_date, platforms: {platform: {signature, url}}}`

## Browser extension pairing

- Chrome/Firefox extension; one-click hand-off of current page → app
- Channel: local axum bridge on `127.0.0.1:<port>` (`local_bridge.rs`)

## Music streaming connectors

- Spotify, SoundCloud, YouTube Music, Qobuz, Last.fm — read-only (likes/playlists alongside local files); Discord Rich Presence for now-playing

## i18n — Weblate

- `i18n/{lang}/{namespace}.json`; translated via hosted.weblate.org/engage/omniget

## Community

- Discord: discord.gg/jgdxyPy7Vn (issues/help/release chatter)
