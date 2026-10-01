# Roadmap

> Part of omniget/.project-knowledge/ | Last updated: 2026-10-01
> Forward-looking only. Check this before starting any task — know what's in flight.

## Current Goal

The project's official home is now **OpenSelena/omniget** (appeared 2026-09-30, same day tonhowtf vanished) — fixes for the app itself go there as PRs via the `YahyaZekry/omniget-1` fork: #6 (AppImage gst env) and #7 (media bridge) are open; fork-only machinery (keyring/secrets, which OpenSelena doesn't have) stays on YahyaZekry/omniget (#1–#3). Then: re-point the in-app updater + plugin-registry endpoints (still dead tonhowtf URLs) at OpenSelena or a mirror we control.

---

## Known Bugs

- [x] ~~Blank page on media open~~ — fixed across two layers: the gst-plugin-path fix stopped the WebKitWebProcess crash, and the media-bridge fix (`fix/webkitgtk-media-bridge`, c9899013) makes local video/audio/subtitles actually stream (WebKitGTK's media loader bypasses Tauri's custom scheme; media now plays from the local bridge with Range support). Built into the AppImage 2026-09-30, playback verified in-app. *(found: 2026-09-30, fixed: 2026-09-30)*

---

## Active TODOs

- [ ] ~~Rebuild the AppImage after upstream merges any changes~~ — obsolete: upstream is gone; rebuild after local fixes instead *(added: 2026-08-16, resolved: 2026-09-30)*
- [ ] ~~Decide whether to install `patchelf` system-wide via pacman~~ — resolved: not needed; linuxdeploy bundles its own patchelf and `pnpm tauri:appimage` succeeds without it *(added: 2026-08-16, resolved: 2026-08-22)*
- [ ] ~~User action: delete the stale broken `~/Applications/omniget.AppImage` copy and reinstall via Gear Lever from the new build~~ — resolved: `~/Applications/omniget.AppImage` no longer exists; current build lives at `~/AppImages/omniget.appimage` *(added: 2026-08-22, resolved: 2026-09-30)*
- [ ] Decide where fixes go now that upstream is gone: push the fix branch (`fix/keyring-runtime-panic`) to the fork, and whether to re-point the updater/plugin-registry endpoints somewhere we control *(added: 2026-09-30)*

---

## Planned Features

- (none beyond upstream backlog in `docs/backlog.md` — note upstream is gone, so the backlog only exists in this clone)
