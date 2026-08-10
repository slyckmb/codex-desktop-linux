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
# Dispatch mode. "background" (default) runs the worker detached with nohup so
# the runner returns immediately and the worker is not killed by a parent
# timeout; a chatrap signal is written on completion. "foreground" blocks.
DISPATCH_MODE="${WATCHDOG_DISPATCH_MODE:-background}"
# Optional chatrap signal delivery: when set, the runner writes ack/done/failed
# signals under this directory (chatrap_signal_dir). Leave empty to disable.
SIGNAL_DIR="${WATCHDOG_SIGNAL_DIR:-}"
# Identifier used for the signal file name (defaults to the DMG sha prefix).
SIGNAL_TASK="${WATCHDOG_SIGNAL_TASK:-watchdog}"
# Email alerting on bail/problems. Recipient (defaults to the updater's email
# config). Set to empty to disable email alerts.
ALERT_EMAIL="${WATCHDOG_ALERT_EMAIL:-}"
# Whether to run the git upstream sync before dispatching the DMG worker.
RUN_REPO_SYNC="${WATCHDOG_RUN_REPO_SYNC:-1}"

log() {
  if [ -n "$LOG_FILE" ]; then
    printf '%s %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$LOG_FILE"
  fi
}

# Write a chatrap-style signal (ack/done/failed) under SIGNAL_DIR. Mirrors
# chatrap_write_signal from chatrap-common.sh. No-op when SIGNAL_DIR is empty.
write_signal() {
  local signal_type="$1" status="$2" log_path="$3"
  [ -n "$SIGNAL_DIR" ] || return 0
  mkdir -p "$SIGNAL_DIR"
  local file="${SIGNAL_DIR}/${SIGNAL_TASK}.${signal_type}"
  {
    printf 'SIGNAL_TYPE=%s\n' "$signal_type"
    printf 'TASK_ID=%s\n' "$SIGNAL_TASK"
    printf 'TIMESTAMP=%s\n' "$(date -u +'%Y-%m-%dT%H:%M:%SZ')"
    printf 'STATUS=%s\n' "$status"
    printf 'LOG_PATH=%s\n' "$log_path"
  } > "$file"
  log "signal=$signal_type task=$SIGNAL_TASK file=$file"
}

# Email the user when the runner bails or hits a problem. Uses the configured
# sendmail/msmtp. No-op when ALERT_EMAIL is empty or msmtp/sendmail is missing.
email_alert() {
  local subject="$1" body="$2"
  [ -n "$ALERT_EMAIL" ] || { log "email_alert skipped (no ALERT_EMAIL)"; return 0; }
  local mta=""
  if command -v msmtp >/dev/null 2>&1; then
    mta="msmtp"
  elif [ -x /usr/sbin/sendmail ]; then
    mta="/usr/sbin/sendmail"
  else
    log "email_alert skipped (no MTA)"
    return 0
  fi
  {
    printf 'To: %s\n' "$ALERT_EMAIL"
    printf 'Subject: %s\n' "$subject"
    printf 'Date: %s\n' "$(date -R)"
    printf 'Content-Type: text/plain; charset=utf-8\n\n'
    printf '%s\n' "$body"
  } | $mta -t -i 2>>"${LOG_FILE:-/dev/null}" || log "email_alert failed to send"
  log "email_alert sent: $subject"
}

# Safely sync the fork with the upstream repo. Runs `make sync-upstream`
# (fetch upstream -> rebase -> push origin) but aborts and does NOT force-push
# if the rebase hits a conflict. Returns 0 on success, non-zero on failure.
# Safe-by-default: never pushes a conflicted/broken rebase.
sync_repo() {
  log "repo sync start"
  if [ "$RUN_REPO_SYNC" != "1" ]; then
    log "repo sync skipped (RUN_REPO_SYNC=$RUN_REPO_SYNC)"
    return 0
  fi
  # Guard: must be on main with a clean tree before rebasing.
  local branch
  branch="$(git -C "$REPO_ROOT" branch --show-current 2>/dev/null || true)"
  if [ -z "$branch" ] || [ "$branch" != "main" ]; then
    log "repo sync aborted (not on main: '$branch')"
    return 3
  fi
  if [ -n "$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null)" ]; then
    log "repo sync aborted (dirty worktree)"
    return 3
  fi

  git -C "$REPO_ROOT" fetch upstream main >> "${LOG_FILE:-/dev/null}" 2>&1 || return 4
  git -C "$REPO_ROOT" rebase --rebase-merges upstream/main >> "${LOG_FILE:-/dev/null}" 2>&1
  local rc=$?
  if [ "$rc" -ne 0 ]; then
    # Abort the conflicted rebase and restore the pre-rebase state. Do NOT push.
    log "repo sync conflict; aborting rebase (no push)"
    git -C "$REPO_ROOT" rebase --abort >> "${LOG_FILE:-/dev/null}" 2>&1 || true
    email_alert "codex-desktop upstream sync conflict" \
      "The upstream git rebase in $REPO_ROOT hit a conflict and was aborted. The DMG worker was NOT launched. Resolve the conflict on main (git rebase --abort was already run) and re-run the watchdog."
    return 5
  fi

  # Push only after a clean rebase (force-with-lease is safe: it won't clobber
  # remote changes we haven't seen).
  git -C "$REPO_ROOT" push --force-with-lease origin main >> "${LOG_FILE:-/dev/null}" 2>&1
  local push_rc=$?
  if [ "$push_rc" -ne 0 ]; then
    log "repo sync push failed (rc=$push_rc)"
    return 6
  fi
  log "repo sync ok"
  return 0
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
  log "dispatching worker for $sha (mode=$DISPATCH_MODE)"
  write_signal "ack" "started" "${LOG_FILE:-unknown}"

  local worker_log="${LOG_FILE:-/tmp/codex-desktop-watchdog-worker.log}"
  local cmd
  if [ "$WORKER_CLI" = "opencode" ]; then
    cmd=(opencode run --dir "$REPO_ROOT" --model "$MODEL" --auto --format json "$prompt")
  else
    cmd=(codex exec -m "$MODEL" -C "$REPO_ROOT" --dangerously-bypass-approvals-and-sandbox "$prompt")
  fi

  # Emit a done/failed signal after the worker process exits.
  run_worker_bg() {
    "${cmd[@]}" >> "$worker_log" 2>&1
    local rc=$?
    if [ "$rc" -eq 0 ]; then
      write_signal "done" "success" "$worker_log"
    else
      write_signal "failed" "exit_${rc}" "$worker_log"
    fi
    return $rc
  }

  if [ "$DISPATCH_MODE" = "background" ]; then
    # Run detached so a parent timeout / service restart does not kill the
    # worker mid-repair. Export the signal + worker config so the background
    # subshell can write completion signals. The signal file records completion
    # for a watcher/lead.
    export SIGNAL_DIR SIGNAL_TASK LOG_FILE worker_log
    export WATCHDOG_CMD_BASH="${cmd[*]}"
    nohup bash -c '
      log() { [ -n "$LOG_FILE" ] && printf "%s %s\n" "$(date -u "+%Y-%m-%dT%H:%M:%SZ")" "$*" >> "$LOG_FILE"; }
      write_signal() {
        local t="$1" s="$2" lp="$3"
        [ -n "$SIGNAL_DIR" ] || return 0
        mkdir -p "$SIGNAL_DIR"
        { printf "SIGNAL_TYPE=%s\n" "$t"; printf "TASK_ID=%s\n" "$SIGNAL_TASK"; \
          printf "TIMESTAMP=%s\n" "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"; \
          printf "STATUS=%s\n" "$s"; printf "LOG_PATH=%s\n" "$lp"; } > "$SIGNAL_DIR/$SIGNAL_TASK.$t"
        log "signal=$t task=$SIGNAL_TASK file=$SIGNAL_DIR/$SIGNAL_TASK.$t"
      }
      eval "${WATCHDOG_CMD_BASH}" >> "$worker_log" 2>&1
      rc=$?
      if [ "$rc" -eq 0 ]; then write_signal "done" "success" "$worker_log";
      else write_signal "failed" "exit_${rc}" "$worker_log"; fi
      exit "$rc"
    ' >> /dev/null 2>&1 &
    log "worker dispatched in background (pid $!)"
  else
    run_worker_bg
  fi
}

main() {
  log "run start"

  # Run the git repo sync and the DMG probe in parallel: both are network-heavy
  # (fetch/rebase upstream + download/check the DMG). The probe downloads the
  # DMG; sync_repo rebases the repo. They are independent.
  local sync_rc probe_out
  local sync_log probe_log
  sync_log="$(mktemp)"; probe_log="$(mktemp)"
  {
    sync_repo
    echo "SYNC_RC=$?" >&2
  } >"$sync_log" 2>&1 &
  local sync_pid=$!
  {
    run_probe
    echo "PROBE_RC=$?" >&2
  } >"$probe_log" 2>&1 &
  local probe_pid=$!

  wait "$probe_pid"
  probe_out="$(grep -vE '^PROBE_RC=' "$probe_log" || true)"
  local probe_rc
  probe_rc="$(grep -oE '^PROBE_RC=[0-9]+' "$probe_log" | cut -d= -f2 || echo 0)"

  wait "$sync_pid"
  sync_rc="$(grep -oE '^SYNC_RC=[0-9]+' "$sync_log" | cut -d= -f2 || echo 0)"

  local sync_lines
  sync_lines="$(grep -vE '^SYNC_RC=' "$sync_log" || true)"
  [ -n "$sync_lines" ] && log "repo sync log: $(echo "$sync_lines" | tr '\n' ' ')"
  rm -f "$sync_log" "$probe_log"

  log "probe output: ${probe_out:-<none>} (probe_rc=$probe_rc)"
  log "repo sync rc=$sync_rc"

  # Gate: the DMG worker builds from origin/main. If the repo sync failed
  # (conflict/abort), do NOT launch the worker on a bad base. Bail + alert.
  if [ -n "$sync_rc" ] && [ "$sync_rc" -ne 0 ]; then
    log "repo sync failed; skipping DMG worker dispatch"
    email_alert "codex-desktop watchdog: repo sync failed" \
      "The upstream git sync failed (rc=$sync_rc); the DMG repair worker was NOT launched. See the runner log: ${LOG_FILE:-<none>}"
    write_signal "failed" "repo_sync_rc_${sync_rc}" "${LOG_FILE:-unknown}"
    return 1
  fi

  # CHANGE_READY <sha> EVENT_ID=<id>: drift detected, dispatch the worker.
  if [[ "$probe_out" =~ CHANGE_READY[[:space:]]+([0-9a-f]{64}) ]]; then
    local sha="${BASH_REMATCH[1]}"
    dispatch_worker "$sha"
    local event_id
    event_id="$(grep -oE 'EVENT_ID=[^ ]+' <<<"$probe_out" | cut -d= -f2)"
    if [ -n "$event_id" ]; then
      python3 "$WATCHDOG" event-ack --event-id "$event_id" "${STATE_ARGS[@]}" >> "$LOG_FILE" 2>&1 || true
      log "acked event $event_id"
    fi
    return 0
  fi

  # WORKER_ACTIVE / NIX_ACTIVE / NIX_REPAIR_READY / CAMPAIGN_WAITING / UNCHANGED:
  # nothing to do this cycle.
  log "no dispatch needed ($probe_out)"

  # ORPHAN RECOVERY (see docstring): recover a campaign whose worker died or
  # whose CHANGE_READY was acked without acquisition. Only after a clean sync.
  if [[ "$probe_out" == *"CAMPAIGN_WAITING"* ]]; then
    local orphan_sha
    orphan_sha="$(python3 "$WATCHDOG" status "${STATE_ARGS[@]}" 2>/dev/null \
      | jq -r '.active_campaign.campaign_phase as $p
             | .active_campaign.sha256 as $sha
             | select($p == "drift-validation" or $p == "detected")
             | select(.worker_lease == null)
             | select((.active_campaign.pr_number // null) == null)
             | select((.active_campaign.repair_rounds // [] | map(select(.pr_number != null or .merge_sha != null or .accepted_head_sha != null)) | length) == 0)
             | $sha' 2>/dev/null || true)"
    if [ -n "$orphan_sha" ]; then
      log "orphaned campaign detected: $orphan_sha; dispatching worker"
      dispatch_worker "$orphan_sha"
    fi
  fi
  return 0
}

main