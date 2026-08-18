# Roadmap

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-18
> Forward-looking only. Check this before starting any task — know what's in flight.

## Current Goal

All open PRs merged (#290 signing key fix, #294 pnpm-workspace.yaml relocation). Fork synced to upstream `main` (`b4774e26`). Next: monitor upstream release for v0.8.6.

---

## Known Bugs

- [ ] linuxdeploy's bundled `strip` fails on newer glibc libs (`.relr.dyn` section) → AppImage bundling via tauri-cli fails on Arch/modern toolchains. Workaround: package AppDir directly with appimagetool; not fixable in-repo (old binary bundled by tauri-cli). *(found: 2026-08-16)*

---

## Active TODOs

- [ ] Rebuild the AppImage after upstream merges any changes (the current `~/OmniGet.AppImage` is from this source tree) *(added: 2026-08-16)*
- [ ] Decide whether to install `patchelf` system-wide via pacman (needed only if AppImage bundling is done through tauri itself) *(added: 2026-08-16)*

---

## Planned Features

- (none beyond upstream backlog in `docs/backlog.md`)
