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


---

# Recommended Execution Order — recovery/sync/build/install (v0.3)

**Author:** Fern (dsv4pro), 2026-08-15
**Purpose:** A mechanically-executable, phase-by-phase sequence for the codex-desktop-linux
recovery goal. Each phase = exact commands + stop/boundary condition + what dsv4pro reviews.

## Live state (verified 2026-08-15 15:42, NOT assumed)
- Branch `cr/openclaw-integration-fixes`, clean tree. Feature HEAD = `72e72a4`.
- `upstream/main` = `e6b51d96` (moved +1 since v0.2: `Fix RPM GnuPG dependency on SUSE (#1363)` — RPM-only, unrelated to conflicts).
- `origin/main` = `fc99c52`; local `main` = `fc99c52`.
- Divergence: **28 ahead / 109 behind** (`git rev-list --left-right --count main...upstream/main`).
- Conflicts: **13** (confirmed `merge-tree --write-tree`): Makefile, docs/updater.md, scripts/install-deps.sh, updater/src/{app,builder,cache_cleanup,cli,config,main,state}.rs, + modify/delete on updater/src/{wrapper,wrapper_apply}.rs and linux-features/codex-wrapper-updater/README.md.
- DMG: repo `Codex.dmg`=33ad47 (Aug 5, stale); watchdog `last_observed`=ecffb6fc (Aug 15), `last_accepted`=f8a5a74a (Aug 9); worker_lease cleared.

## The three ordering decisions (answered up front)

**(a) DMG choice — decide EARLY (Phase 2), but it only gates the BUILD (Phase 6), not the sync.**
The app binary comes from the DMG (extracted by `install.sh`), NOT compiled from repo source. So the DMG choice is independent of the sync and can be locked early without blocking anything. Recommend `f8a5a74a` (last-accepted) unless Michael wants the bleeding-edge `ecffb6fc`.

**(b) Updater-model decision — FIRST (Phase 1), it is the linchpin.**
Adopt upstream's signed-Linux-package model vs keep the fork's wrapper-updater. This single decision determines (1) how all 13 conflicts resolve — especially the 10 `updater/src/` content conflicts and 3 modify/deletes — and (2) what `cargo build -p codex-update-manager` actually produces. It must be made BEFORE any conflict resolution. It is a human architectural call, not a mechanical merge.

**(c) Sync/force-push BEFORE build/install — sync first, always.**
The updater binary is compiled from repo source, so it MUST build from synced code (not 109-behind main). Building the updater before syncing would produce an immediately-stale binary and force a rebuild. The app (from DMG) is sync-independent, but the updater is not. Sync → build → install is the only correct order.

---

## RECOMMENDED EXECUTION ORDER

### Phase 0 — Safety snapshot & baseline (no decisions, pure safety)
- [ ] `cd /home/michael/dev/tools/codex-desktop-linux`
- [ ] `git status --porcelain` → must be empty (except ignored artifacts). Abort if dirty.
- [ ] `git tag -a backup/prework-20260815 main -m "pre-sync baseline"`
- [ ] `git tag -a backup/prework-20260815-feature HEAD -m "pre-sync feature baseline"`
- [ ] Record checkpoint SHAs in the plan doc: origin/main=fc99c52, upstream/main=e6b51d96, feature HEAD=72e72a4, divergence=28/109, conflict=13.
- **Boundary:** dsv4pro confirms tags exist + tree clean + SHAs recorded.

### Phase 1 — Updater-model decision (THE linchpin — human decision)
- [ ] Present Michael the adopt-vs-keep choice with evidence (upstream `47f6d69` deleted the fork's wrapper subsystem; 13 fork-only files).
- [ ] Michael decides: **ADOPT upstream signed-package model** (recommended) or **KEEP fork wrapper model**.
- [ ] Record the decision + rationale in the plan doc.
- **Boundary:** Michael decides; dsv4pro reviews the decision's downstream implications (which conflicts resolve which way).

### Phase 2 — DMG choice (independent; lock early)
- [ ] Choose: `f8a5a74a` (accepted) / `ecffb6fc` (newest observed) / fresh download via `install.sh`.
- [ ] Record choice + SHA. If using a watchdog DMG, `cp` it to repo root as `Codex.dmg`.
- **Boundary:** dsv4pro confirms DMG choice + SHA recorded; build will use this exact artifact.

### Phase 3 — Sync main onto upstream (resolve 13 conflicts per Phase 1 decision)
- [ ] `git checkout main && git fetch upstream`
- [ ] `git rebase --rebase-merges upstream/main` (stops at first conflict).
- [ ] Resolve all 13 conflicts grouped: updater/src (10 content) + modify/deletes (3) per Phase 1; Makefile (keep fork's sync-upstream targets, take upstream build/feature); install-deps.sh (keep fork's dpkg robustness, take upstream run_privileged); docs/updater.md (rewrite to winning model).
- [ ] `git log` sanity: upstream history intact, fork's 28 commits preserved, no fork-only file silently lost.
- **Boundary (BLOCKER):** Michael reviews the resolved `git diff` + updater-model outcome; dsv4pro reviews before any push.

### Phase 4 — Force-push main (safety-guarded)
- [ ] Record pre-push SHA = `fc99c522a0bf959b29808b1667eb03009534eb31`.
- [ ] `git push --force-with-lease=fc99c522a0bf959b29808b1667eb03009534eb31 origin main`
- **Boundary:** dsv4pro confirms push succeeded + `origin/main` == local `main` == resolved tree.

### Phase 5 — Rebase feature branch onto main
- [ ] `git checkout cr/openclaw-integration-fixes && git rebase main`
- [ ] Resolve any new conflicts (watchdog commits are small; upstream may touch scripts/automation).
- **Boundary:** dsv4pro reviews the feature-branch rebase result.

### Phase 6 — Build (from synced code + chosen DMG)
- [ ] `./install.sh ./Codex.dmg` → `codex-app/` (watch DMG extract warnings, Electron/node download).
- [ ] `cargo build --release -p codex-update-manager` (updater from synced source).
- [ ] `./scripts/build-deb.sh` → `dist/*.deb`.
- [ ] Validate: `bash -n` on shell scripts; `cargo test -p codex-update-manager`; confirm `.deb` produced + launcher markers.
- **Boundary:** dsv4pro reviews build outputs + confirms not silently using a stale DMG.

### Phase 7 — Install + updater service
- [ ] Install the built `.deb` (or validate readiness without disrupting a running app).
- [ ] Enable/confirm the systemd user updater service per `packaging/linux/` hooks.
- [ ] Verify installed version/candidate state.
- **Boundary:** dsv4pro reviews install state + service.

### Phase 8 — Final verification + sitrep + commit
- [ ] `git status` clean; commit remaining docs.
- [ ] Verify watchdog still probes cleanly (no regression from sync/build).
- [ ] Write results + next-steps to `docs/`; commit.
- [ ] Final dsv4pro review + plain-language sitrep to Michael.

---

## Risk summary
- **History rewrite** (Phase 4) is the highest-risk step; guarded by backup tags + `--force-with-lease=<sha>` + Michael BLOCKER in Phase 3.
- **Updater-model decision** (Phase 1) is the highest-judgment step; must precede conflict resolution.
- **Stale DMG** (Phase 2) must be forced; building from repo `Codex.dmg` (Aug 5) deviates from watchdog state.
- **Order is non-negotiable:** sync → build → install (updater compiles from source; building pre-sync = wasted work).

_v0.3 — execution order; dsv4pro review only, no sync/build/force-push executed._

---

## Phase 0 checkpoint (executed 2026-08-15 15:4x)
- Backup tags: `backup/prework-20260815` (main), `backup/prework-20260815-feature` (feature HEAD)
- origin/main=`fc99c52` upstream/main=`e6b51d9` local main=`fc99c52` feature HEAD=`c5c5b09`
- Divergence: 28 ahead / 109 behind
- Conflicts (merge-tree): **13**
- Tree clean (tracked).

---

## Phase 1 & 2 decisions (Michael, 2026-08-15 15:47)
- **Updater model: ADOPT upstream** signed-Linux-package model. Fork wrapper-updater architecture surrendered; conflicts resolve toward upstream for updater/*, with fork-only wrapper files deleted. Rationale: lowest conflict + lowest forever-maintenance; recommended by dsv4pro.
- **DMG: f8a5a74a** (last-accepted by watchdog, Aug 9). Build from this exact artifact.

---

## Phase 3 EXECUTED (2026-08-15)
- Rebased `main` onto upstream/main: **28 ahead / 0 behind** (was 28/109).
- Resolved all **13 conflicts** per ADOPT-upstream: updater/src/* adopted theirs (wrapper.rs/wrapper_apply.rs/codex-wrapper-updater deleted), install-deps.sh adopted upstream run_privileged + consolidated funcs, Makefile kept fork sync targets + adopted upstream structure.
- Removed untracked fork-wrapper leftovers. Tree clean.
- Smoke: bash -n install.sh/install-deps.sh OK; make parses OK.
- NOTE: `main` is BRANCH-PROTECTED (no direct commits). Force-push to origin is the next gated step and REQUIRES Michael OK.
