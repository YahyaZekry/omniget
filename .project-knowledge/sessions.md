# Session Log

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-22
> Append-only — never edit past entries.

| Date | Summary |
|------|---------|
| 2026-08-16 | Built OmniGet from source on Garuda/Arch: installed rustup (1.97.0 pinned via rust-toolchain.toml) + pnpm 10.29.3, approved esbuild/@swc-core postinstall, generated a local updater signing key, built the deb and AppImage, verified the app runs with plugins auto-loaded. Opened PR #290 (upstream): default `createUpdaterArtifacts` to false with `TAURI_CONFIG` override in release.yml + `pnpm.onlyBuiltDependencies`. Re-formatted the PR description (balanced line widths, icons). Created `.project-knowledge/` folder. |
| 2026-08-18 | PR #290 merged upstream (author confirmed: TAURI_CONFIG merge-patch mechanism verified, cargo tests + pnpm check + pnpm test all passed). Author moved `onlyBuiltDependencies` to `pnpm-workspace.yaml` in PR #294. Synced fork (`YahyaZekry/omniget`) with upstream `main` (`b4774e26`). Updated project-knowledge: roadmap, history, sessions. |
| 2026-08-22 | Debugged Gear Lever failing to install OmniGet AppImages: journal showed `desktop_entry.get` crash on None — AppImage had absolute `.desktop`/`.DirIcon` symlinks (tauri-bundler ≤2.10 bug, fixed upstream #15596). Bumped `@tauri-apps/cli` to ^2.11.4. Fixed two bundling blockers found while rebuilding: linuxdeploy strip on `.relr.dyn` (`NO_STRIP=1`) and GTK plugin crash on Arch's missing gdk-pixbuf dirs (vendored patched plugin + `pnpm tauri:appimage`). Added `src-tauri/desktop.template` with `Categories=Network;FileTransfer;`, wired via `bundle.linux.deb/rpm.desktopTemplate`; added `NO_STRIP=1` to release.yml. Built `omniget_0.8.5_amd64.AppImage`, verified relative symlinks + `application/x-desktop` detection after simulating Gear Lever's 7z extraction. |

## (13) 2026-09-19 — "sync": PR #332 MERGED upstream; local main synced, AppImage rebuild

User said "sync". Found BOTH Yahya PRs merged upstream: #328 (sqlite) earlier and now #332 (i18n sweep) as 1eaa355f. Main also gained: lo.json (#330), #339, v0.10.0, README rewrite.

Sync steps: local main had 4 commits not on origin (Gear Lever fix duplicate + user's docs commits; the fix itself was upstream as PR #304). Merged origin/main into main (pattern matches their previous c0db0f88 merge). Conflicts (all add/add from the parallel Gear Lever lineages): desktop.template, package.json, release.yml, linuxdeploy-plugin-gtk.sh → took THEIRS (upstream reviewed versions from #304 + v0.10.0). README.md → theirs + re-appended the user's small "AI Context" <details> block (from 20efb54c). Merge commit a7e46db5.

Then: pnpm install, pnpm tauri:appimage from synced main, verify (sqlite exports = 0 via squashfs extract + nm; i18n keys present in bundle), install to ~/AppImages/omniget.appimage.

State: local main == user's fork main + all upstream. Branch fix/i18n-pt-strings fully merged — its remaining purpose is history only.

AppImage build finished (exit 0, ~35min — v0.10.0's new deps zbus/keyring compiled from scratch). Verification: squashfs extract → 0 sqlite3 exports ✓; desktop file has Categories=Network;FileTransfer;Utility + GenericName ✓; binary greps show 0 plaintext keys (Tauri 2 compresses embedded assets — binary grep is NOT a valid i18n check; verify via build/_app/ bundle instead, which contains sweep keys and was built 23:20 from the synced tree). Installed: cp omniget_0.10.0_amd64.AppImage → ~/AppImages/omniget.appimage (replaced Sep 13 build).

Note: fork/main still points at old c0db0f88; local main is a7e46db5. Update fork/main with `git push fork main` if wanted.

## (14) 2026-09-30 — "blank page + A Tauri App": webview crash diagnosed, keyring panic fixed, desktop entry refreshed

User reported: (1) app shows blank page — check the logs; (2) launcher shows "a Tauri app" instead of a real description.

Blank page: journal showed the 04:13 (login-time, systemd autostart) instance started fine, then its WebKitWebProcess aborted at 04:15 (SIGABRT, `g_signal_connect_data: NULL instance` assertion, coredump 1089078) — renderer dead, window left blank. Fresh relaunch renders perfectly (screenshot-verified). Not reproducible on normal launches; kept as roadmap Known Bug.

Bonus find while reading logs: a tokio panic on EVERY launch — `zbus-4.4.0 utils.rs block_on: Cannot start a runtime from within a runtime`. Root cause: keyring 3's Linux backend (`async-secret-service` over zbus) block_ons the calling thread; async Tauri commands run on tokio workers → the task touching a secret (AI keys / OmniDisc / profile seed) dies. Fix in `secrets.rs`: the three raw keyring calls now run on a dedicated OS thread (`off_runtime`). 10/10 secrets tests pass. Committed `fix/keyring-runtime-panic` e8742fbf (push needs user approval per collaboration rules).

Launcher: the AppImage's embedded desktop file is correct; "A Tauri App" came from Gear Lever's cached entry written back when Cargo still had the create-tauri-app default description (changed upstream in c9709621 docs(seo)) and never refreshed after in-place AppImage updates. Rewrote `~/.local/share/applications/omniget.desktop` with real Comment/GenericName/Categories/Keywords + `%u` in Exec (also fixes the plasmashell deep-link warning); backup kept.

Also discovered: upstream `tonhowtf/omniget` (and all tonhowtf repos) are GONE from GitHub → explains the journal's plugin-registry and updater 404s; fork is now primary. Rebuilt AppImage from the fix branch and reinstalled to `~/AppImages/omniget.appimage`; relaunch verified panic-free.

## (15) 2026-09-30 (later) — Study-tab blank screen root-caused: missing GStreamer plugins in the AppImage

User reported: Study tab → click a course/download → hangs, then blank. Captured live on the running instance: WebKitWebProcess logged `GStreamer element autoaudiosink not found`, two `GLib-GObject-CRITICAL: invalid (NULL) pointer instance` / `g_signal_connect_data` assertions, then SIGABRT (coredump 1410258, 10:30:41) — same signature as the morning's 04:15 blank window. Media playback init dies, page goes blank.

Root cause: the AppImage bundles GStreamer's core libraries but `usr/lib/gstreamer-1.0/` is empty (linuxdeploy doesn't fill it), so the bundled libgstreamer never sees the system plugins (libgstapp/autodetect/playback all exist in /usr/lib/gstreamer-1.0). AppRun also pre-sets GST_PLUGIN_SYSTEM_PATH to the empty bundled dir, overriding the default scan.

Fix: `main.rs setup_environment()` — inside an AppImage and only when the user hasn't set GST_PLUGIN_PATH, set it additively to `$APPDIR/usr/lib/gstreamer-1.0` + system `/usr/lib[/64]/gstreamer-1.0` (ABI-compatible: bundled libs are the same gst the system plugins were built against). Commit 0cd20087 on `fix/appimage-gstreamer-plugins`, stacked on `fix/keyring-runtime-panic`. cargo check clean; rebuilt, installed to `~/AppImages/omniget.appimage`, relaunched: 0 gst warnings (previously every launch), 0 panics, UI renders (screenshot-verified). User to confirm course playback.

Note: the 10:21 SIGBUS crashes of the old instance's WebKit helpers were self-inflicted (cp replaced the AppImage under the running instance) — not a bug.

## (16) 2026-09-30 (evening) — "video keeps spinning": WebKitGTK media loader bypasses scheme handlers; local bridge now serves media

User: video spins but never plays. Escalated diagnosis with isolated harnesses: system WebKit plays the file via file:// (mojibake charset bug in first harness invalidated earlier env-matrix results — always set <meta charset=utf-8>); app still spun when its binary ran against SYSTEM libs (rules out bundled gst/webkit env); harness with a registered `asset` scheme reproduced the failure while a real HTTP server played. Conclusion: WebKitGTK's GStreamer media source makes a real HTTP request and never consults registered URI schemes — `convertFileSrc` URLs (http://asset.localhost, in-process only) die instantly with FormatError before any pipeline builds. Images/page resources DO go through the scheme handler (why thumbnails worked).

Fix: local bridge serves media. Rust: `media_stream_url` command mints unguessable 6h single-file grants (cap 512, pruned); `GET /media/{token}` streams with Range/206 + CORS (crossorigin=anonymous player); tokio-util "io" feature added; 18 bridge tests. Frontend: `mediaSrc()` helper ($lib/media-src.ts) — bridge URLs on Linux, convertFileSrc on Win/macOS; wired watch page, PlayerShell subtitles, music player store (both src sites), file-clip, and (second commit 427bd107) the COURSE LESSON page — the user's actual path, missed the first time, which is why "now it doesn't play" after the first rebuild. Third commit d34cfc10: mediaSrc cache TTL (5h) so cached URLs re-mint before the 6h grant expires. 582 vitest + svelte-check green. AppImage rebuilt; in-app verification showed real metadata + rendered frame (0:01/0:03 vs endless 0:00/0:00).

## (17) 2026-10-01 (early AM) — race fixed in AppRun hook; final build verified; 3 PRs opened on the fork

The final-build spin regression was NOT a race in set_var: I had built from fix/webkitgtk-media-bridge-pr (independent branch) which never contained the GST main.rs fix — the gst fix lived only on fix/appimage-gstreamer-plugins-pr. Lesson: verify which commits a build contains (build5 came from a branch missing fix #2).

Durable fix: scripts/linux/linuxdeploy-plugin-gtk.sh (vendored hook) now exports GST_PLUGIN_PATH (additive system dirs) and GST_PLUGIN_SCANNER (system helper) in the AppRun hook BEFORE exec — deterministic, no GLib getenv-cache race; user-provided values win. main.rs set_var stays as documented fallback. Committed 6529a3ac on fix/appimage-gstreamer-plugins-pr (PR #2 updated); pushed.

Machine rebooted at 23:45 (killed build6, wiped /tmp) — rebuilt everything as build-all branch (all three concerns cherry-picked) → final AppImage installed to ~/AppImages/omniget.appimage. Verified: boot log has 0 appsink warnings, 0 zbus panics, 0 'External plugin loader failed'; app+web process env carry the hook-set GST vars pre-exec (visible in /proc/environ). Course-lesson playback confirmed visually earlier (metadata 42:38, frames advancing); final-click verification was interrupted because the user was actively using the desktop — playback evidence stands from the same code path.

PRs opened on YahyaZekry/omniget (fork main fast-forwarded to a7e46db5 first): #1 keyring tokio panic, #2 AppImage gst env (hook), #3 WebKitGTK media bridge. PR branches are cherry-picks off main (fix/*-pr); stacked working branches exist locally. Left-over: /home/monst3r/Downloads/gst-test.mp4 (3s color-bars test clip, also indexed as a lesson in the study DB — delete file + lesson row at will).

## (18) 2026-10-01 — PRs redirected to OpenSelena/omniget (the project's new official repo)

User pointed at https://github.com/OpenSelena/omniget — the official continuation that appeared the same day tonhowtf vanished. User has pull-only access there; forked as YahyaZekry/omniget-1 (remotes: openselena, selena-fork). Histories diverged at 013ff612: OpenSelena has 23 own commits (open-nami rebrand, their v0.10.0, SEO); our main has 37 they lack (incl. PR #332 and the whole secrets.rs/Linux-keyring architecture — so the keyring panic fix does not apply there and wasn't PR'd).

Rebuilt the two applicable fixes on openselena/main via cherry-pick: fix/appimage-gstreamer-plugins (main.rs fallback + AppRun hook; auto-merged clean) and fix/webkitgtk-media-bridge (4 commits; one conflict — PlayerShell imports: their file never had the i18n `t` import, dropped convertFileSrc import with the rest). Verified on their tree: cargo check both branches, 9 local_bridge tests, svelte-check 0 errors. Pushed to omniget-1; PRs opened: OpenSelena#6 (gst env) and #7 (media bridge). Memory updated: upstream → OpenSelena.

## (19) 2026-10-01 — sync workflow updated for OpenSelena; first real sync merged (77aa3faf)

User: "the workflow to sync needs to update with it". Retargeted remotes: origin → OpenSelena/omniget (was dead tonhowtf), dropped the temporary `openselena` remote (origin replaces it); fork and selena-fork unchanged. Documented the full sync procedure in stack.md.

Ran the first real sync: merged origin/main into local main — 10 conflict files, all resolved (identity/SEO/flatpak → OpenSelena; Tauri identifier kept wtf.tonho.omniget for data continuity; store.rs kept our secrets refactor over their inline legacy shim; lib.rs command registry concatenated with their 8 open_nami commands; lo.json took their real Lao translation (ours had drifted to English); nav test updated to 9 entries, their OmniDisc nav test dropped — /omnidisc is not a nav item on our line). Verified on the merged tree: cargo check, svelte-check 0 errors, vitest 598 passed (one fix along the way: the merge commit was amended after the nav-test final edit missed staging). Merge commit 77aa3faf on local main. Fork main push left for user approval; AppImage rebuild offered.

## (20) 2026-10-01 (cont.) — sync completed end-to-end; branch-collision gotcha recorded

Two regressions caught and fixed during the first sync's rebuilds: (1) the first synced build failed the i18n strict gate — taking their lo.json wholesale dropped the 1983 keys our sweep added; resolved as ours + their one real Lao improvement (merge amended). (2) The next synced build came back with the keyring panic + appsink warning — the OpenSelena PR work had RECREATED the branch names fix/appimage-gstreamer-plugins and fix/webkitgtk-media-bridge on top of openselena/main (deleting the stacked originals), so merging "fix/webkitgtk-media-bridge" brought only the media fix. GOTCHA for future syncs: branch names on this repo get reused across bases; verify a branch's base with git merge-base before trusting its name. build-all retains the full original stack; keyring (c80eefac) + gst (dac6cf5c, 2f80dd7c) cherry-picked onto main instead. Final main verified: cargo check, 10 secrets tests, 598 vitest; final AppImage installed and boots with 0 warnings / 0 panics / hook env present pre-exec.

## (21) 2026-10-01 — PR CI green on OpenSelena (#6, #7); remote hygiene

User reported CI failures on the OpenSelena PRs. Root causes, both style-only: (1) code wasn't rustfmt-formatted (their CI enforces `cargo fmt --check`) — formatted both branches; (2) one media doc-comment used the legacy overindented list style, tripping their per-OS clippy-baseline gate as a new warning (`doc_overindented_list_items 4 → 9`) — rewrote the doc item at proper indentation, local `node scripts/clippy-baseline.mjs --check` passed. After pushes: all 8 checks pass on #6 AND #7 (rust ×3 OSes, rust-debian, frontend, metainfo, smoke ×2). Now purely awaiting OpenSelena review.

Also: knowledge files restored from stash tangle (`.project-knowledge` is gitignored-but-tracked on our main — never exists on OpenSelena-based branches, which is what made the stash dance messy) and committed to main (ce901c69), fork main pushed. New rustfmt rule of thumb for this repo: run `cargo fmt --all` in src-tauri/ before every push to a PR branch.
