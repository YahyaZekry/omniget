# History

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-22
> Past-only. Append-only — never delete entries.

## Removed

- *(none)*

---

## Fixed

- **Updater signing key required for every source build** → defaulted `bundle.createUpdaterArtifacts` to `false` in `tauri.conf.json`, re-enabled only for tagged releases via `TAURI_CONFIG` in `release.yml` (PR #290). Verified: `pnpm tauri build --bundles deb` succeeds with no signing env vars. *(fixed: 2026-08-16)*
- **pnpm onlyBuiltDependencies in wrong file (PR #290)** → `pnpm` field in `package.json` is dead config on pnpm 10; author relocated to `pnpm-workspace.yaml` in PR #294, now upstream. *(fixed: 2026-08-16, relocated upstream: 2026-08-17)*
- **linuxdeploy AppImage bundling failed** (strip + missing FUSE) → worked around locally with `APPIMAGE_EXTRACT_AND_RUN=1` and by running appimagetool directly on the assembled AppDir; produced working `~/OmniGet.AppImage`. *(fixed: 2026-08-16, workaround only)*
- **AppImage couldn't run without FUSE2** → worked around with `--appimage-extract-and-run` / libfuse2 note. *(fixed: 2026-08-16)*
- **Gear Lever failed to install OmniGet AppImages** (`AttributeError: 'NoneType' object has no attribute 'get'`) → tauri-bundler 2.10.0 created **absolute** `.desktop`/`.DirIcon` symlinks in the AppDir; Gear Lever extracts with 7z, which turns absolute links into dangling ones, so no desktop entry is found. Root fix: bump `@tauri-apps/cli` to ^2.11.4 (upstream PR #15596 makes both symlinks relative). *(fixed: 2026-08-22)*
- **linuxdeploy strip failure on `.relr.dyn`** (supersedes the 2026-08-16 appimagetool workaround) → linuxdeploy's bundled `strip` is too old for DT_RELR sections in current Arch libs and any strip failure aborts bundling. Proper fix: `NO_STRIP=1`, set by the new `pnpm tauri:appimage` script and in `release.yml`. Fresh linuxdeploy downloads don't help — upstream still bundles old binutils. *(fixed: 2026-08-22)*
- **Tauri GTK plugin fails on Arch** (`cp: cannot stat '/usr/lib/gdk-pixbuf-2.0/2.10.0'`) → Arch ships gdk-pixbuf with loaders built in but stale pkg-config paths; plugin `copy_tree` + cache write then fail under `set -e`. Fix: vendored patched plugin at `scripts/linux/linuxdeploy-plugin-gtk.sh` (skip missing sources, mkdir -p before loaders.cache), auto-synced to `~/.cache/tauri/` by `pnpm tauri:appimage`. *(fixed: 2026-08-22)*

---

## Decisions

- **`createUpdaterArtifacts: false` as repo default** — source builders shouldn't need maintainer secrets; releases opt back in via env config instead of the committed file. Keeps the runtime updater pubkey in config so installed apps still verify updates. *(2026-08-16)*
- **pnpm approval committed to `pnpm-workspace.yaml`** rather than `package.json`/`.npmrc`/interactive — makes installs deterministic under pnpm 10. (Note: `package.json` `pnpm` field is dead config on pnpm 10.29.3.) *(2026-08-16)*
- **`.project-knowledge/` left untracked** — this clone's `origin` is the upstream repo we PR into; knowledge files must not leak into the PR branch. *(2026-08-16)*
- **AppImage packaging fixes live in-repo** (`pnpm tauri:appimage` wrapper + vendored GTK plugin + desktop template) instead of manual appimagetool post-processing — reproducible for anyone building on Arch, no extra manual steps. Desktop template also gives the AppImage proper `Categories=Network;FileTransfer;`. *(2026-08-22)*
