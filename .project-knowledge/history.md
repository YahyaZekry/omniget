# History

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-18
> Past-only. Append-only — never delete entries.

## Removed

- *(none)*

---

## Fixed

- **Updater signing key required for every source build** → defaulted `bundle.createUpdaterArtifacts` to `false` in `tauri.conf.json`, re-enabled only for tagged releases via `TAURI_CONFIG` in `release.yml` (PR #290). Verified: `pnpm tauri build --bundles deb` succeeds with no signing env vars. *(fixed: 2026-08-16)*
- **pnpm onlyBuiltDependencies in wrong file (PR #290)** → `pnpm` field in `package.json` is dead config on pnpm 10; author relocated to `pnpm-workspace.yaml` in PR #294, now upstream. *(fixed: 2026-08-16, relocated upstream: 2026-08-17)*
- **linuxdeploy AppImage bundling failed** (strip + missing FUSE) → worked around locally with `APPIMAGE_EXTRACT_AND_RUN=1` and by running appimagetool directly on the assembled AppDir; produced working `~/OmniGet.AppImage`. *(fixed: 2026-08-16, workaround only)*
- **AppImage couldn't run without FUSE2** → worked around with `--appimage-extract-and-run` / libfuse2 note. *(fixed: 2026-08-16)*

---

## Decisions

- **`createUpdaterArtifacts: false` as repo default** — source builders shouldn't need maintainer secrets; releases opt back in via env config instead of the committed file. Keeps the runtime updater pubkey in config so installed apps still verify updates. *(2026-08-16)*
- **pnpm approval committed to `pnpm-workspace.yaml`** rather than `package.json`/`.npmrc`/interactive — makes installs deterministic under pnpm 10. (Note: `package.json` `pnpm` field is dead config on pnpm 10.29.3.) *(2026-08-16)*
- **`.project-knowledge/` left untracked** — this clone's `origin` is the upstream repo we PR into; knowledge files must not leak into the PR branch. *(2026-08-16)*
