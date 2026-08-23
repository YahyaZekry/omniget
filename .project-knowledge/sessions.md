# Session Log

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-22
> Append-only — never edit past entries.

| Date | Summary |
|------|---------|
| 2026-08-16 | Built OmniGet from source on Garuda/Arch: installed rustup (1.97.0 pinned via rust-toolchain.toml) + pnpm 10.29.3, approved esbuild/@swc-core postinstall, generated a local updater signing key, built the deb and AppImage, verified the app runs with plugins auto-loaded. Opened PR #290 (upstream): default `createUpdaterArtifacts` to false with `TAURI_CONFIG` override in release.yml + `pnpm.onlyBuiltDependencies`. Re-formatted the PR description (balanced line widths, icons). Created `.project-knowledge/` folder. |
| 2026-08-18 | PR #290 merged upstream (author confirmed: TAURI_CONFIG merge-patch mechanism verified, cargo tests + pnpm check + pnpm test all passed). Author moved `onlyBuiltDependencies` to `pnpm-workspace.yaml` in PR #294. Synced fork (`YahyaZekry/omniget`) with upstream `main` (`b4774e26`). Updated project-knowledge: roadmap, history, sessions. |
| 2026-08-22 | Debugged Gear Lever failing to install OmniGet AppImages: journal showed `desktop_entry.get` crash on None — AppImage had absolute `.desktop`/`.DirIcon` symlinks (tauri-bundler ≤2.10 bug, fixed upstream #15596). Bumped `@tauri-apps/cli` to ^2.11.4. Fixed two bundling blockers found while rebuilding: linuxdeploy strip on `.relr.dyn` (`NO_STRIP=1`) and GTK plugin crash on Arch's missing gdk-pixbuf dirs (vendored patched plugin + `pnpm tauri:appimage`). Added `src-tauri/desktop.template` with `Categories=Network;FileTransfer;`, wired via `bundle.linux.deb/rpm.desktopTemplate`; added `NO_STRIP=1` to release.yml. Built `omniget_0.8.5_amd64.AppImage`, verified relative symlinks + `application/x-desktop` detection after simulating Gear Lever's 7z extraction. |
