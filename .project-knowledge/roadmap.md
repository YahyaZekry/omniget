# Roadmap

> Part of omniget/.project-knowledge/ | Last updated: 2026-08-22
> Forward-looking only. Check this before starting any task — know what's in flight.

## Current Goal

Local Linux packaging pipeline fully working: AppImages build via `pnpm tauri:appimage` and install cleanly through Gear Lever. Next: monitor upstream release for v0.8.6.

---

## Known Bugs

- (none currently tracked)

---

## Active TODOs

- [ ] ~~Rebuild the AppImage after upstream merges any changes~~ — done 2026-08-22 (`omniget_0.8.5_amd64.AppImage`, Gear Lever-verified) *(added: 2026-08-16, done: 2026-08-22)*
- [ ] ~~Decide whether to install `patchelf` system-wide via pacman~~ — resolved: not needed; linuxdeploy bundles its own patchelf and `pnpm tauri:appimage` succeeds without it *(added: 2026-08-16, resolved: 2026-08-22)*
- [ ] User action: delete the stale broken `~/Applications/omniget.AppImage` copy and reinstall via Gear Lever from the new build *(added: 2026-08-22)*

---

## Planned Features

- (none beyond upstream backlog in `docs/backlog.md`)
