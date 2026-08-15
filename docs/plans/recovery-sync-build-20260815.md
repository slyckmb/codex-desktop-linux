# Codex Desktop Linux — Recovery, Sync, Build & Install Plan (v0.2)

**Author:** Fern (OpenClaw agent), reviewed by dsv4pro
**Date:** 2026-08-15
**Goal:** Bring the codex-desktop-linux repo fully up to date with upstream, build a working Codex
Linux app (and updater) from the current upstream DMG, install it, and leave the repo committed,
clean, and documented — reviewed by dsv4pro at every phase boundary.

> **v0.2 changes** — see the [dsv4pro review](#dsv4pro-review-v02) section at the bottom. The most
> important corrections: the claim of a "two-file conflict" is **wrong** (13 conflicts incl. a
> deleted updater subsystem), and the DMG the plan cited (`f0264157`) is **stale** and not the repo's
> `Codex.dmg`.

## Target Outcome / Definition of Done
1. `main` (fork) is rebased onto current `upstream/main`, the merge conflicts are resolved **correctly &
   deliberately** (this is the hard, high-risk step), and the result is pushed with
   `--force-with-lease`. The active feature branch is rebased onto the clean main.
2. A full Linux build succeeds from a **current upstream DMG** via `./install.sh <dmg>`, producing
   `codex-app/` + the Debian `.deb` + the updater binary.
3. The built `.deb` is installed (or install is validated ready) and the updater service enabled.
4. Repo committed, clean, documented; sitrep + next-steps written.
5. Every phase boundary independently reviewed by dsv4pro; feedback committed before proceeding.

## Ground Truth (verified live, 2026-08-15 ~15:25)
- Repo: `/home/michael/dev/tools/codex-desktop-linux`; branch `cr/openclaw-integration-fixes`, clean tree.
- Remotes: `origin=git@github.com:slyckmb/codex-desktop-linux.git`, `upstream=git@github.com:ilysenko/…`.
- `main` = `origin/main` = local `fc99c522`; `upstream/main` = `f362dfe0` (`Separate Chronicle Skysight as an opt-in feature (#1354)`).
- Divergence: **28 commits ahead / 108 behind upstream** (`git rev-list --left-right --count main...upstream/main` -> `28 108`). The plan's "108 behind" was correct but omitted the 28 fork-ahead commits (which are the fork's own work that must be preserved).
- **The conflict is NOT two files.** `git merge-tree --write-tree origin/main upstream/main` reports **13 conflicts**, including the entire `updater/src/` subsystem:
  - content: `Makefile`, `docs/updater.md`, `scripts/install-deps.sh`
  - content (updater): `updater/src/{app,builder,cache_cleanup,cli,config,main,state}.rs`
  - **modify/delete:** `updater/src/wrapper.rs`, `updater/src/wrapper_apply.rs`, `linux-features/codex-wrapper-updater/README.md` — upstream DELETED these; the fork modified them.
  - **Root cause:** upstream commit `47f6d69 "refactor: rebuild updates from signed Linux packages"` **removed the entire fork's wrapper-updater architecture** (13 fork-only files: `wrapper.rs`, `wrapper_apply.rs`, `cli_management.rs`, `codex_cli.rs`, `changelog.rs`, `diagnostics.rs`, `feature_picker.rs`, `feature_snapshot.rs`, `npm_cli_repair.rs`, `operational_guard.rs`, `outbox.rs`, `restart.rs`, + tests). The fork's updater is a **custom wrapper-updater**; upstream replaced it with a **signed-package rebuild** model. This is an architectural merge, not a textual one.
  - Also across `linux-features/`: fork has dirs upstream lacks (`codex-wrapper-updater`, `ssh-command-wrapper`, `conversation-mode`, `deferred-update-build`, `open-target-discovery`, `record-and-replay`, `x11-ewmh-computer-use`); upstream has dirs the fork lacks (`automation-extensions`, `chronicle-skysight`, `computer-use-linux`, `linux-performance-workarounds`, `nix-store-bundled-marketplace-permissions`, `compatibility.json`). These are non-conflicting additions (both sides added different dirs), but must be merged, not lost.
- **DMG: the plan's claim is wrong.** Repo `Codex.dmg` = sha `33ad4701…` (618,651,816 B ≈ 618.7 MB, dated **Aug 5**) — this does **not** match any watchdog DMG. The `f0264157…` DMG the plan cites is the watchdog's **Aug 10** download (556 MB), and it is now **stale**. Watchdog downloads today: `ecffb6fc` (Aug 15, newest observed, 609 MB) and accepted `f8a5a74a` (Aug 9, 572 MB). **Decision needed: which DMG to build from.**
- Build tooling present: python3, 7z/p7zip, curl, unzip, tar, flock, make, g++, dpkg-deb, cargo 1.94.1, node v24.19.0. **Missing:** rpmbuild, pacman, appimagetool → scope to **Debian (.deb)** + updater; rpm/pacman/AppImage are out of scope.
- Watchdog: active campaign `drift-validation`, base/head `fc99c52`; **leases stale**: `worker_lease` expired 2026-08-14T19:25, `nix_merge_lease` null. `check_upstream.state` = `28/108`.
- `install.sh` accepts a positional DMG path (parsed in `scripts/lib/install-helpers.sh` → `PROVIDED_DMG_PATH`); DMG is gitignored.

---

## Phase 0 — Safety Snapshot & Baseline (Mandatory)
Purpose: a guaranteed rollback point before any history rewrite.
- [ ] `git tag -a backup/prework-20260815 main` and `git tag -a backup/prework-20260815-feature cr/openclaw-integration-fixes` (annotated, with message).
- [ ] Record in `docs/plans/recovery-sync-build-20260815.md` (this file) a checkpoint block: SHAs (`origin/main`=`fc99c52`, `upstream/main`=`f362dfe0`, feature branch HEAD=`e62ff34`), the DMG decision from Phase 1 step 0, `cargo`/`node`/tool versions.
- [ ] Verify clean worktree (`git status --porcelain` empty except ignored artifacts); abort if dirty.
- [ ] **Snapshot remote state** to catch upstream drift during the rebuild: `git fetch upstream` and record `upstream/main` SHA again. If it moved, re-confirm the divergence numbers.
- **Boundary:** dsv4pro reviews baseline + this checkpoint before proceeding.

## Phase 1 — Sync `main` onto upstream (the real conflict-resolution work)
> This is the highest-risk step. The plan previously framed it as a 2-file fix; it is a **13-file
> conflict including a deleted updater subsystem**. Do NOT hand-merge blindly — decide per-region.

**Step 1.0 — Pick the build DMG first (decision, not an afterthought).**
The repo `Codex.dmg` is stale (Aug 5). Options (choose one, record it):
- (a) **Use the watchdog's newest accepted DMG** (`f8a5a74a`, 572 MB) — the last one upstream marked acceptable.
- (b) **Use the newest observed** (`ecffb6fc`, Aug 15, 609 MB) — latest, but **not yet accepted** by watchdog.
- (c) Re-download fresh via `install.sh`'s own fetch (no positional arg) — latest from upstream URL, but re-downloads ~600 MB and may be unaccepted.
- Record the chosen DMG path + SHA in the checkpoint, and copy it as `Codex.dmg` in the repo root if (a) or (b). **Rationale:** build from the repo's stale Aug 5 DMG would produce an app the watchdog already superseded.

**Step 1.1 — fetch + rebase onto upstream.**
- [ ] `cd /home/michael/dev/tools/codex-desktop-linux && git checkout main && git fetch upstream`
- [ ] `git rebase --rebase-merges upstream/main` (this will stop at the first conflict).
- [ ] **Resolve the 13 conflicts deliberately** — this is the core decision, grouped:
  - **Updater subsystem (highest impact):** Decide whether to (i) **adopt upstream's signed-package model** and let the fork's wrapper files be deleted (favored — upstream owns the updater; the fork's `cli_management`, `wrapper.rs`, `outbox`, `notification` bits are fork-specific and 108+ commits removed from upstream), or (ii) **keep the fork's wrapper model** (merge fork files back in — high maintenance burden). Recommend (i) adopt upstream, keeping ONLY genuine fork improvements (e.g. the fork's `c2352ea "durable email notification"` — port that as a patch on the new model rather than resurrecting dead files).
  - **Makefile:** reconcile the fork's extra targets (`sync-upstream`, `assert-downstream-branch`, `--rebase-merges`) with upstream's changes. Keep fork's `sync-upstream` helpers (they're the fork's own workflow); take upstream's build/feature changes.
  - **scripts/install-deps.sh:** merge fork's dpkg/GUI-prompt robustness (`5cdcb8a`, `677e5a8`, `8f5e415` — broken-dpkg tolerance) with upstream's `run_privileged` + node/npm/rustup install. These fork commits are durable value; keep them.
  - **docs/updater.md:** rewrite to describe whichever updater model won (1i vs 1ii).
  - **modify/delete (`wrapper.rs`, `wrapper_apply.rs`, `codex-wrapper-updater/README.md`):** confirm delete (adopt upstream) unless you chose option (1ii).
- [ ] After the rebase completes cleanly, **`git log` sanity check**: confirm upstream history is intact, the fork's 28 commits are preserved, and no fork-only file is silently lost. Use `git diff` before submit.

**Step 1.2 — force-push with safety.**
- [ ] Critical safety: before pushing, record the **pre-force-push** `main` SHA (`fc99c52`) and the backup tag. Use `git push --force-with-lease=<expected>` where `<expected>` = `fc99c52` so the push only proceeds if remote hasn't moved (safer than bare `--force-with-lease`, which the Makefile `push-origin` uses).
- [ ] `git push --force-with-lease=fc99c522a0bf959b29808b1667eb03009534eb31 origin main` — but **only after Michael's eyes on the resolved diff** (below).
- **Boundary (BLOCKER):** Do NOT push until Michael reviews the conflict-resolution `git diff` + the updater-model decision. Then dsv4pro reviews before push.

**Step 1.3 — rebase the feature branch.**
- [ ] `git checkout cr/openclaw-integration-fixes && git rebase main`.
- [ ] Resolve any new conflicts (the watchdog commits on this branch are small; upstream may have touched `scripts/automation/` or the watchdog files).
- [ ] **Boundary:** dsv4pro reviews the feature-branch rebase result.

## Phase 2 — Full build from the current DMG
- [ ] `./install.sh ./Codex.dmg` (using the Phase 1.0 DMG) → `codex-app/`. Watch for: DMG extract warnings (`/Applications` symlink), node-runtime install, native-module rebuild, Electron download.
- [ ] Build the updater: `cargo build --release -p codex-update-manager` (Makefile target equivalent; `CARGO_JOBS_ARG` supported).
- [ ] Build the Debian package: `./scripts/build-deb.sh` → `dist/*.deb`.
- [ ] Validate: `codex-app/start.sh` exists, launcher markers present, `.deb` produced. Run `bash -n install.sh scripts/lib/*.sh scripts/build-deb.sh` and `cargo test -p codex-update-manager` for smoke confidence.
- [ ] **Boundary:** dsv4pro reviews build outputs, patch-drift/warnings, and that the build is not silently using a stale DMG.

## Phase 3 — Install & updater service
- [ ] Install the built `.deb` (or validate install readiness without disrupting a running app).
- [ ] Enable/confirm the systemd user updater service (`codex-update-manager.service`) per `packaging/linux/` hooks.
- [ ] Verify installed version / candidate state via the updater.
- **Boundary:** dsv4pro reviews install state + service.

## Phase 4 — Final verification, docs, sitrep
- [ ] `git status` clean; commit remaining docs.
- [ ] Verify the watchdog still probes cleanly (no regression from the sync/build). Note: watchdog leases were stale before this work; confirm the next probe run isn't blocked by the post-sync state.
- [ ] Write results + next-steps to `docs/` and commit.
- [ ] Final dsv4pro review + plain-language sitrep to Michael.

---

## Risks / Notes
- **History rewrite:** main force-push is deliberate and irreversible-ish; backup tags + `--force-with-lease=<sha>` guard. Without `<sha>`, bare lease only checks last-fetched.
- **Updater architecture divergence is the real risk,** not the Makefile. Decide signed-package (adopt) vs wrapper (keep) explicitly. Don't "resolve" 13 conflicts mechanically.
- **Stale repo DMG:** building from `Codex.dmg` (Aug 5) deviates from what the watchdog now tracks. Phase 1.0 forces an explicit choice.
- **108-behind + 28-ahead:** plain `git pull --rebase upstream main` would lose the 28 fork commits if mishandled. `--rebase-merges` + careful resolution is required.
- **Watchdog lease:** worker lease expired ~24h ago; if the phase work overlaps a watchdog worker dispatch, the watchdog agent (dedicated, per prior work) may re-run. Keep the repo state reviewable so a mid-sync probe doesn't act on a half-resolved tree.

## Out of Scope
- rpm / pacman / AppImage packaging (host lacks rpmbuild/pacman/appimagetool).
- Merging the "ChatGPT Community"/Chronicle Skysight rebrand as a product decision beyond keeping the build green — capture divergence explicitly for Michael.

---

## dsv4pro review (v0.2)
Changes made (with evidence):
1. **Corrected the conflict scope (was the biggest error).** v0.1 claimed a "two-file conflict (Makefile + install-deps.sh)". Verified with `git merge-tree --write-tree origin/main upstream/main` → **13 conflicts**, including the whole `updater/src/` subsystem and modify/delete on the fork's wrapper files. Upstream `47f6d69 "refactor: rebuild updates from signed Linux packages"` deleted the fork's wrapper-updater architecture; 13 fork-only files are now gone upstream. Added explicit updater-model decision (adopt vs keep) as the real work item.
2. **Corrected the DMG claim.** v0.1 said `Codex.dmg` = `f0264157…`. Repo `Codex.dmg` = `33ad4701…` (Aug 5), and `f0264157` is the watchdog's stale Aug 10 download (556 MB). Watchdog now tracks `ecffb6fc` (Aug 15) / accepted `f8a5a74a` (Aug 9). Added Phase 1.0 to force a DMG choice.
3. **Documented the 28-ahead / 108-behind divergence** (v0.1 only said "108 behind"). The 28 fork commits must be preserved; flagged the silent-loss risk.
4. **Hardened the force-push step:** replaced bare `--force-with-lease` with `--force-with-lease=<pre-push sha>` and made Michael's review a hard BLOCKER before push.
5. **Noted the linux-features dir asymmetry** (fork-only + upstream-only feature dirs) so the merge doesn't silently drop either side's additions.
6. **Added a DMG-freshness/acceptance tie-in** to Phase 2 and a watchdog-probe regression check to Phase 4.

_v0.2 — reviewed & hardened; ready for Michael + dsv4pro phase-0 sign-off._
