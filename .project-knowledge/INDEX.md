# OmniGet — Knowledge Index

> Last updated: 2026-08-18
> Status: Active
> Stack: Tauri 2 (Rust) + SvelteKit 2 / Svelte 5 · pnpm 10 · rusqlite (SQLite)
> Current goal: Get PR #290 (source builds without updater signing key) merged upstream

## What This Project Does
OmniGet is a free, open-source desktop download manager (Windows/macOS/Linux) that downloads online courses, videos, music, and books from 1,800+ sites using yt-dlp — with a built-in course player, EPUB/PDF reader, music library, notes app, and a plugin system. Your files stay local. (Repo: tonhowtf/omniget)

---

## Files in This Folder

| File | Contents | Load when... |
|------|----------|--------------|
| `stack.md` | Tech stack, dev commands, env vars | Setting up, adding deps, running builds |
| `structure.md` | File tree, entry points, key files | Navigating the codebase |
| `systems.md` | Engine, plugins, updater, bridge, music/reader/study | Touching any cross-cutting system |
| `features.md` | User-facing features + workflows | Understanding what's built |
| `integrations.md` | yt-dlp, FFmpeg, plugins, updater, cookies, extensions | External systems & data contracts |
| `roadmap.md` | Bugs, TODOs, current goal | Starting any task |
| `history.md` | Fixes, decisions | Debugging, reviewing past decisions |
| `sessions.md` | Session-by-session log | Reviewing work history |

Not created (deemed unnecessary): `schema.md` (raw rusqlite, schema lives in `src-tauri/src/core/db.rs`), `hooks.md` (Svelte stores, no React hooks), `components.md` (component/design rules are already authoritative in `AGENTS.md`).

---

## Context Loading Guide

| Task | Load these files |
|------|-----------------|
| Building / installing | `stack.md` |
| Download engine work | `systems.md` + `features.md` |
| Plugin development | `systems.md` + `integrations.md` + `docs/plugin-development.md` |
| Frontend UI work | `structure.md` + `AGENTS.md` (design system) |
| Fixing a bug | `roadmap.md` + relevant area file |
| General orientation | This file → then pick by task |
| Full audit | All files |

---

*Maintained with [project-knowledge](https://github.com/YahyaZekry/claude-code-skills) · by [Yahya Zekry](https://github.com/YahyaZekry)*
