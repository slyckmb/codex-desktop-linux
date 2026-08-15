# Codex Desktop Linux — Full Recovery, Sync, Build & Install Plan

**Author:** Fern (OpenClaw agent)
**Date:** 2026-08-15
**Goal:** Bring the codex-desktop-linux repo fully up to date with upstream, build a working
Codex Linux app (and updater) from the current DMG, install it, and leave the repo committed,
clean, and documented — with every phase reviewed by dsv4pro before the next phase starts.

## Target Outcome / Definition of Done
1. `main` (fork) is synced onto current `upstream/main` after resolving the two-file conflict,
   pushed with `--force-with-lease`, and the active feature branch is rebased onto clean main.
2. A full Linux build of codex-desktop-linux succeeds from the latest DMG (`f0264157…`) via the
   documented path (`./install.sh ./Codex.dmg`), producing `codex-app/` + the Debian `.deb`
   (+ updater target build).
3. The built app is installed (or install is validated as ready), and the updater service is
   enabled.
4. The repo is committed, working tree clean, and a sitrep + next-steps is written to the repo.
5. Every phase boundary is committed and independently reviewed by dsv4pro, whose feedback is
   incorporated (committed) before proceeding.

## Ground Truth (verified live, 2026-08-15)
- Repo: `/home/michael/dev/tools/codex-desktop-linux`; branch `cr/openclaw-integration-fixes`,
  working tree clean.
- Remotes: `origin=git@github.com:slyckmb/codex-desktop-linux.git`, `upstream=ilysenko/…`.
- `main` = `origin/main` = local `fc99c52`; **behind upstream by 108 commits**.
- Dry-run rebase (origin/main onto upstream/main) conflicts in **`Makefile`** (1 large region)
  and **`scripts/install-deps.sh`** (2 regions). Bumped to 108 behind since last check.
- `Codex.dmg` present (618 MB) = watchdog-tracked SHA `f02641573285…`.
- Build tooling present: python3, 7z/p7zip, curl, unzip, tar, flock, make, g++, dpkg-deb, cargo,
  node v24.19.0. **Missing:** rpmbuild, pacman, appimagetool → scope native build to **Debian
  (.deb)** + updater; note others as optional.
- Watchdog active campaign: `drift-validation`, head `fc99c52`; lease stale/expired.

---

## Phase 0 — Safety Snapshot & Baseline
- Create a git ref/branch backup of current `main` + feature branch HEADs so any phase can be
  rolled back (no data loss). e.g. `git tag backup/pre-work-20260815` and note the DMG/sha.
- Record starting SHAs, tree hashes, and env into a `docs/` checkpoint file.
- Verify clean tree; abort if dirty.
- **Boundary:** dsv4pro review of baseline + Phase 0 result.

## Phase 1 — Sync main onto upstream (the real conflict resolution)
- On `main`: `git fetch upstream`, then `git rebase --rebase-merges upstream/main`.
- Resolve `Makefile` conflict (keep fork's extended targets + knowingly reconcile/absorb the
  upstream "ChatGPT Community" restructure or document the divergence decision).
- Resolve `scripts/install-deps.sh` (merge fork's dpkg/GUI-prompt robustness with upstream's
  `run_privileged` + node/npm/rustup install).
- After clean rebase: `git push --force-with-lease origin main` (this is the deliberate,
  history-rewriting step — reconcile only after Michael's eyes on the resolved diff).
- Rebase the active feature branch `cr/openclaw-integration-fixes` onto the new clean main.
- **Boundary:** stop, run a build smoke check, dsv4pro reviews the conflict-resolution diff.

## Phase 2 — Full build from current DMG
- Run `./install.sh ./Codex.dmg` (the documented regenerate path) → produces `codex-app/`.
- Build the updater: `cargo build --release -p codex-update-manager`.
- Build the Debian package: `./scripts/build-deb.sh` → `dist/*.deb`.
- Validate: `codex-app/start.sh` exists, launcher markers, package produced. (Optionally:
  rpm/pacman/appimage marked OUT-OF-SCOPE — missing host tooling.)
- **Boundary:** dsv4pro reviews build outputs + any patch-drift or warnings.

## Phase 3 — Install & updater service
- Install the built `.deb` (or validate install readiness without disrupting a running app).
- Enable the updater service (systemd user): confirm `codex-update-manager.service` enabled.
- Verify installed version / candidate state via updater.
- **Boundary:** dsv4pro reviews install state + service.

## Phase 4 — Final verification, docs, sitrep
- `git status` clean; commit any remaining docs.
- Verify the watchdog still probes cleanly (no regression from the sync).
- Write the results + next-steps to `docs/` (or a sitrep file) and commit.
- Final dsv4pro review of Phase 4 + whole plan outcomes; then plain-language sitrep to Michael.

---

## Risks / Notes
- **History rewrite:** main force-push is deliberate; backup tag + `--force-with-lease` guards.
- **Build scope:** Debian/.deb + updater only on this host (rpm/pacman/AppImage need missing
  tools — flagged, not failure).
- **Watchdog lease:** stale lease noted; not blocking build/sync.
- Sync must precede build (build from current code, not 108-behind main).

## Out of Scope
- rpm / pacman / AppImage packaging (host lacks rpmbuild/pacman/appimagetool).
- Merging upstream "ChatGPT Community" rebrand as a product decision beyond keeping the build
  green — capture any divergence explicitly in docs for Michael to own.

_Version 0.1 (initial) — to be reviewed/hardened by dsv4pro._
