#!/usr/bin/env bash
# Runs the upstream DMG watchdog directly from the repository, outside any
# Codex automation sandbox. On a detected drift (CHANGE_READY) it invokes a
# model call to run the repair Worker flow. Designed to be driven by a systemd
# timer or cron.
#
# The watchdog state machine lives in watchdog.py and is the single source of
# truth. This runner only (1) probes, (2) reacts to what the probe emits, and
# (3) hands repair work to a model Worker. It never re-implements campaign
# phase logic.
set -euo pipefail

# Resolve the repository root (parent of this script's directory, .../scripts/automation/upstream-dmg-watchdog).
SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." && pwd)"
WATCHDOG="$SCRIPT_DIR/watchdog.py"

# Optional overrides (systemd can set these via the unit file).
STATE_DIR="${WATCHDOG_STATE_DIR:-}"
MODEL="${WATCHDOG_MODEL:-cline-pass/cline-pass/deepseek-v4-flash}"
REASONING="${WATCHDOG_REASONING:-minimal}"
LOG_FILE="${WATCHDOG_LOG_FILE:-}"
PROBE_INTERVAL_SECONDS="${WATCHDOG_PROBE_INTERVAL_SECONDS:-3600}"
# CLI used to dispatch the repair worker. opencode is the default (cheap
# deepseek models, no read-only bubblewrap sandbox). Override with
# WATCHDOG_WORKER_CLI=codex to fall back to codex exec.
WORKER_CLI="${WATCHDOG_WORKER_CLI:-opencode}"

log() {
  if [ -n "$LOG_FILE" ]; then
    printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$LOG_FILE"
  fi
}

STATE_ARGS=()
if [ -n "$STATE_DIR" ]; then
  STATE_ARGS+=(--state-dir "$STATE_DIR")
fi

# The model Worker prompt. This is the full 9-step repair flow from
# docs/upstream-dmg-watchdog.md. It is invoked via `codex exec` so the model has
# full local filesystem access (no read-only bubblewrap sandbox).
read -r -d '' WORKER_PROMPT <<'PROMPT' || true
You are the dedicated Worker for the codex-desktop-linux upstream DMG watchdog at REPO_ROOT. Follow docs/upstream-dmg-watchdog.md exactly. A CHANGE_READY event was emitted for upstream DMG SHA: SHA256. Repair it:

1. Acquire the campaign with: python3 scripts/automation/upstream-dmg-watchdog/watchdog.py worker-acquire --sha SHA256
2. Create a managed worktree from current origin/main; record it with campaign-update.
3. Run sync-features once with the user's primary checkout as --source-checkout.
4. Build the candidate from the campaign DMG. Re-target each drifted patch contract in scripts/patches/impl/ until they match the new upstream code, commit the source change, record the new head, build again, and call record-acceptance --decision FILE --head HEAD (requires an accepted verdict).
5. For a changed source head, run nix-preflight. For an unchanged accepted main, skip it.
6. Open one repair PR whose body contains <!-- upstream-dmg-sha256:SHA256 -->.
7. Wait for the six merge gates (Rust and Smoke Tests, Debian, RPM, Pacman, Nix Package Builds, Build App Against Upstream DMG), run validate-repair-pr immediately before merge.
8. After the merge, call advance-to-nix --pr-number NUMBER (omit --pr-number for unchanged accepted main).
9. Use immutable sync-features, commit before record-acceptance, pass nix-preflight, require all six gates, and never merge without the deterministic guards. Follow the feature-only fast path when every changed path is inside the affected linux-features/<id>/ directories.

Report a sitrep with the PR URL and acceptance verdict when work occurred.
PROMPT

run_probe() {
  python3 "$WATCHDOG" probe "${STATE_ARGS[@]}" 2>&1
}

dispatch_worker() {
  local sha="$1"
  local prompt
  prompt="$(printf '%s' "$WORKER_PROMPT" | sed "s|REPO_ROOT|$REPO_ROOT|g; s|SHA256|$sha|g")"
  log "dispatching worker for $sha"
  # Run the model Worker non-interactively with full local filesystem access.
  # opencode (default) runs with full permissions (--auto) and cheap deepseek
  # models; codex exec is the fallback (--dangerously-bypass-approvals-and-sandbox
  # because the default read-only bubblewrap sandbox forbids repo writes).
  if [ "$WORKER_CLI" = "opencode" ]; then
    opencode run --dir "$REPO_ROOT" --model "$MODEL" --auto --format json \
      "$prompt" >> "$LOG_FILE" 2>&1 || log "opencode worker finished with non-zero exit for $sha"
  else
    codex exec -m "$MODEL" -C "$REPO_ROOT" --dangerously-bypass-approvals-and-sandbox "$prompt" \
      >> "$LOG_FILE" 2>&1 || log "codex worker finished with non-zero exit for $sha"
  fi
}

main() {
  log "probe start"
  local output
  output="$(run_probe)"
  log "probe output: $output"

  # CHANGE_READY <sha> EVENT_ID=<id>: drift detected, dispatch the worker.
  if [[ "$output" =~ CHANGE_READY[[:space:]]+([0-9a-f]{64}) ]]; then
    local sha="${BASH_REMATCH[1]}"
    dispatch_worker "$sha"
    # Acknowledge the event only after the worker has been dispatched.
    local event_id
    event_id="$(grep -oE 'EVENT_ID=[^ ]+' <<<"$output" | cut -d= -f2)"
    if [ -n "$event_id" ]; then
      python3 "$WATCHDOG" event-ack --event-id "$event_id" "${STATE_ARGS[@]}" >> "$LOG_FILE" 2>&1 || true
      log "acked event $event_id"
    fi
    return 0
  fi

  # WORKER_ACTIVE / NIX_ACTIVE / NIX_REPAIR_READY / CAMPAIGN_WAITING / UNCHANGED:
  # nothing to do this cycle.
  log "no dispatch needed ($output)"

  # ORPHAN RECOVERY: a CAMPAIGN_WAITING result can hide a campaign whose
  # CHANGE_READY was acknowledged without a worker ever acquiring it. Detect
  # that orphaned condition and dispatch the worker directly so the drift does
  # not go unrepaired. Idempotent: the campaign's attempts/lease/nix state are
  # all inspected, and the worker will no-op if the campaign is not acquirable.
  if [[ "$output" == *"CAMPAIGN_WAITING"* ]]; then
    local orphan_sha
    orphan_sha="$(python3 "$WATCHDOG" status "${STATE_ARGS[@]}" 2>/dev/null \
      | jq -r '.active_campaign.campaign_phase as $p
             | .active_campaign.sha256 as $sha
             | select($p == "drift-validation" or $p == "detected")
             | select(.worker_lease == null)
             # Orphaned: no real repair work started (no head/worktree/PR), and
             # any round is only a stale 'active' shell left by a dead worker.
             | select((.active_campaign.head_sha // null) == null)
             | select((.active_campaign.worktree // null) == null)
             | select((.active_campaign.pr_number // null) == null)
             | select((.active_campaign.repair_rounds // [] | map(select(.worktree == null and .head_sha == null and .pr_number == null)) | length) == (.active_campaign.repair_rounds // [] | length))
             | $sha' 2>/dev/null || true)"
    if [ -n "$orphan_sha" ]; then
      log "orphaned campaign detected: $orphan_sha; dispatching worker"
      dispatch_worker "$orphan_sha"
    fi
  fi
  return 0
}

main