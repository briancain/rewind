#!/usr/bin/env bash
# scripts/run-canaries.sh — run the full on-demand canary validation sweep across both regions.
#
# Order: shallow (both regions) → setup (both, idempotent) → deep (both regions). Each step blocks
# on the in-cluster Job (scripts/canary.sh already `kubectl wait`s + dumps logs), then the next runs.
# Every step's output is tee'd to /tmp/canary-<tier>-<region>-<timestamp>.log, and a PASS/FAIL
# summary prints at the end. On-demand runs do NOT enable the suspended CronJob schedules.
#
# Usage:
#   ./scripts/run-canaries.sh                      # full sweep, both regions
#   SKIP_SETUP=1 ./scripts/run-canaries.sh         # skip setup (accounts already registered)
#   REGIONS="us-west-2" ./scripts/run-canaries.sh  # single region
#   SKIP_DEEP=1 ./scripts/run-canaries.sh          # shallow only (read-only, non-mutating)
#
# Note: deep is mutating (seeds a video, exercises auth/social/streaming, then deletes it via the
# real cascade and verifies reclamation). It self-cleans. setup is required once per region before
# deep (creates the canary-credentials secret + registers the owner/viewer accounts).

# NOT `set -e`: we want to run every step and summarize, even if one fails.
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CANARY="$ROOT/scripts/canary.sh"
REGIONS="${REGIONS:-us-west-2 us-east-2}"
SKIP_SETUP="${SKIP_SETUP:-0}"
SKIP_DEEP="${SKIP_DEEP:-0}"
TS="$(date +%Y%m%d-%H%M%S)"

if [ ! -x "$CANARY" ]; then
  echo "✗ $CANARY not found or not executable" >&2
  exit 1
fi

declare -a RESULTS
FAILED=0

run_step() {
  local tier="$1" region="$2"
  local log="/tmp/canary-${tier}-${region}-${TS}.log"
  echo ""
  echo "════════════════════════════════════════════════════════════════════"
  echo "▶ canary ${tier} @ ${region}    (log: ${log})"
  echo "════════════════════════════════════════════════════════════════════"

  REGION="$region" "$CANARY" "$tier" 2>&1 | tee "$log"

  # canary.sh exits 0 regardless of the canary's own pass/fail, so derive the result from its
  # printed markers (see scripts/canary.sh: trigger_run / run_setup_job).
  local result
  case "$tier" in
    setup)
      if grep -q "Setup complete" "$log"; then result="OK"; else result="CHECK"; fi
      ;;
    *)
      if grep -qi "preconditions failed" "$log"; then
        result="FAIL (preconditions — run setup?)"
      elif grep -q "run PASSED" "$log"; then
        result="PASS"
      elif grep -q "did not complete successfully" "$log"; then
        result="FAIL"
      else
        result="UNKNOWN (check log)"
      fi
      ;;
  esac
  case "$result" in FAIL*|UNKNOWN*) FAILED=1 ;; esac
  RESULTS+=("$(printf '%-8s %-10s %s' "$tier" "$region" "$result")  →  $log")
}

# 1) shallow — read-only liveness (health + feed + search + region-routing). No creds needed.
for r in $REGIONS; do run_step shallow "$r"; done

# 2) setup — idempotent; required for deep. Skip with SKIP_SETUP=1.
if [ "$SKIP_SETUP" != "1" ] && [ "$SKIP_DEEP" != "1" ]; then
  for r in $REGIONS; do run_step setup "$r"; done
fi

# 3) deep — full multi-actor journey + cascade verify. Skip with SKIP_DEEP=1.
if [ "$SKIP_DEEP" != "1" ]; then
  for r in $REGIONS; do run_step deep "$r"; done
fi

echo ""
echo "════════════════════════════════════════════════════════════════════"
echo "CANARY SWEEP SUMMARY  ($TS)"
echo "════════════════════════════════════════════════════════════════════"
for line in "${RESULTS[@]}"; do echo "  $line"; done
echo "════════════════════════════════════════════════════════════════════"

if [ "$FAILED" -ne 0 ]; then
  echo "✗ One or more canary steps did not pass — inspect the /tmp logs above."
  exit 1
fi
echo "✓ All canary steps passed."
