#!/bin/bash
# scripts/status.sh — Platform health overview
set -e

PROFILE="rewind"
# Region is selected via the REGION env var (default us-west-2) so the same script reports either
# region's health: `REGION=us-east-2 ./scripts/status.sh`. Mirrors deploy.sh.
REGION="${REGION:-us-west-2}"
CLUSTER="rewind-dev"
PREFIX="rewind-dev"
QUEUE_NAME="${PREFIX}-transcode-jobs"
SEARCH_QUEUE_NAME="${PREFIX}-search-index-events.fifo"
SEARCH_DLQ_NAME="${PREFIX}-search-index-events-dlq.fifo"

GREEN="\033[32m"
RED="\033[31m"
YELLOW="\033[33m"
CYAN="\033[36m"
BOLD="\033[1m"
RESET="\033[0m"

echo -e "${BOLD}📺 Rewind Platform Status (${REGION})${RESET}"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# --- Pod health ---
# Point kubectl at THIS region's cluster before reading pods: both clusters share the name
# "rewind-dev", so without this the pod health would reflect whatever context is currently selected
# rather than $REGION (mismatching the AWS-side sections below). Mirrors deploy.sh.
aws eks update-kubeconfig --name "$CLUSTER" --region "$REGION" --profile "$PROFILE" >/dev/null 2>&1 || true
PODS=$(kubectl get pods -n rewind --no-headers 2>/dev/null)

# Report the health of long-running workloads (the request-serving services + SQS-worker
# Deployments). CronJob pods (canary, transcode-reconcile, delete-cleanup-reconcile) run to
# completion and leave Completed/Succeeded pods in the CronJob history — those are *successful*
# terminal states, not unhealthy services, so they must NOT count against health (the old
# `grep 1/1 Running` flagged every one of them). A pod is healthy when it is Running with all of
# its containers ready (READY column n/n — not a hardcoded 1/1, so multi-container pods are handled).
# Pods that genuinely failed (Error / CrashLoopBackOff / Pending, incl. a *failed* scheduled run)
# are still surfaced below — a failed CronJob run is worth knowing about.
PODSTATS=$(echo "$PODS" | awk '
  NF < 3                              { next }   # skip blank / malformed lines (e.g. no pods)
  $3 == "Completed" || $3 == "Succeeded" { completed++; next }
  { total++
    split($2, r, "/")
    if ($3 == "Running" && r[2] > 0 && r[1] == r[2]) healthy++
    else unhealthy = unhealthy sprintf("               ⚠️  %s (%s)\n", $1, $3)
  }
  END { printf "%d %d %d\n%s", healthy, total, completed, unhealthy }')
COUNTS=$(echo "$PODSTATS" | head -n1)
HEALTHY=$(echo "$COUNTS" | cut -d' ' -f1)
TOTAL=$(echo "$COUNTS" | cut -d' ' -f2)
COMPLETED=$(echo "$COUNTS" | cut -d' ' -f3)
UNHEALTHY=$(echo "$PODSTATS" | tail -n +2)

if [ "${TOTAL:-0}" -eq 0 ]; then
  echo -e "  Services:    ${YELLOW}no service pods found${RESET}"
elif [ "$HEALTHY" -eq "$TOTAL" ]; then
  echo -e "  Services:    ${GREEN}${HEALTHY}/${TOTAL} healthy${RESET}"
else
  echo -e "  Services:    ${RED}${HEALTHY}/${TOTAL} healthy${RESET}"
  printf "%s" "$UNHEALTHY"
fi
if [ "${COMPLETED:-0}" -gt 0 ]; then
  echo -e "               ${CYAN}${COMPLETED} scheduled job pod(s) completed${RESET}"
fi

# --- Alarms ---
ALARMS_JSON=$(aws cloudwatch describe-alarms --alarm-name-prefix "$PREFIX" \
  --profile "$PROFILE" --region "$REGION" \
  --query "MetricAlarms[].StateValue" --output json 2>/dev/null)
ALARM_OK=$(echo "$ALARMS_JSON" | grep -c '"OK"' || true)
ALARM_FIRE=$(echo "$ALARMS_JSON" | grep -c '"ALARM"' || true)
ALARM_INSUF=$(echo "$ALARMS_JSON" | grep -c '"INSUFFICIENT_DATA"' || true)

if [ "$ALARM_FIRE" -gt 0 ]; then
  echo -e "  Alarms:      ${GREEN}${ALARM_OK} OK${RESET} · ${RED}${ALARM_FIRE} ALARM${RESET} · ${ALARM_INSUF} insufficient"
else
  echo -e "  Alarms:      ${GREEN}${ALARM_OK} OK${RESET} · 0 alarm · ${ALARM_INSUF} insufficient"
fi

echo ""
echo -e "${BOLD}📊 Last 5 minutes:${RESET}"

# --- SQS ---
SQS_ATTRS=$(aws sqs get-queue-attributes \
  --queue-url "https://sqs.${REGION}.amazonaws.com/$(aws sts get-caller-identity --profile $PROFILE --query Account --output text)/${QUEUE_NAME}" \
  --attribute-names ApproximateNumberOfMessages ApproximateNumberOfMessagesNotVisible \
  --profile "$PROFILE" --region "$REGION" --output json 2>/dev/null)
SQS_VISIBLE=$(echo "$SQS_ATTRS" | python3 -c "import sys,json; print(json.load(sys.stdin)['Attributes']['ApproximateNumberOfMessages'])" 2>/dev/null || echo "?")
SQS_INFLIGHT=$(echo "$SQS_ATTRS" | python3 -c "import sys,json; print(json.load(sys.stdin)['Attributes']['ApproximateNumberOfMessagesNotVisible'])" 2>/dev/null || echo "?")
echo "  SQS queue:   ${SQS_VISIBLE} pending · ${SQS_INFLIGHT} in-flight"

# --- Search index sync pipeline ---
ACCOUNT=$(aws sts get-caller-identity --profile "$PROFILE" --query Account --output text 2>/dev/null)
SEARCH_QUEUE_URL="https://sqs.${REGION}.amazonaws.com/${ACCOUNT}/${SEARCH_QUEUE_NAME}"
SEARCH_DLQ_URL="https://sqs.${REGION}.amazonaws.com/${ACCOUNT}/${SEARCH_DLQ_NAME}"

SI_ATTRS=$(aws sqs get-queue-attributes \
  --queue-url "$SEARCH_QUEUE_URL" \
  --attribute-names ApproximateNumberOfMessages ApproximateNumberOfMessagesNotVisible \
  --profile "$PROFILE" --region "$REGION" --output json 2>/dev/null)
SI_VISIBLE=$(echo "$SI_ATTRS" | python3 -c "import sys,json; print(json.load(sys.stdin)['Attributes']['ApproximateNumberOfMessages'])" 2>/dev/null || echo "?")
SI_INFLIGHT=$(echo "$SI_ATTRS" | python3 -c "import sys,json; print(json.load(sys.stdin)['Attributes']['ApproximateNumberOfMessagesNotVisible'])" 2>/dev/null || echo "?")
echo "  Search sync: ${SI_VISIBLE} pending · ${SI_INFLIGHT} in-flight"

DLQ_DEPTH=$(aws sqs get-queue-attributes \
  --queue-url "$SEARCH_DLQ_URL" \
  --attribute-names ApproximateNumberOfMessages \
  --profile "$PROFILE" --region "$REGION" \
  --query "Attributes.ApproximateNumberOfMessages" --output text 2>/dev/null || echo "?")
if [ "$DLQ_DEPTH" = "0" ]; then
  echo -e "  Search DLQ:  ${GREEN}0${RESET} (index in sync)"
elif [ "$DLQ_DEPTH" = "?" ]; then
  echo -e "  Search DLQ:  ${YELLOW}unknown${RESET}"
else
  echo -e "  Search DLQ:  ${RED}${DLQ_DEPTH} dead-lettered ⚠️  (index drifting from table)${RESET}"
fi

# --- DynamoDB counts ---
VIDEOS=$(aws dynamodb scan --table-name "${PREFIX}-videos" --select COUNT \
  --filter-expression "#s = :pub" --expression-attribute-names '{"#s":"status"}' \
  --expression-attribute-values '{":pub":{"S":"published"}}' \
  --profile "$PROFILE" --region "$REGION" --query "Count" --output text 2>/dev/null || echo "?")
USERS=$(aws dynamodb scan --table-name "${PREFIX}-users" --select COUNT \
  --profile "$PROFILE" --region "$REGION" --query "Count" --output text 2>/dev/null || echo "?")
echo "  Videos:      ${VIDEOS} published"
echo "  Users:       ${USERS} registered"

echo ""

# --- Active alarms ---
if [ "$ALARM_FIRE" -gt 0 ]; then
  echo -e "${RED}${BOLD}🚨 Active alarms:${RESET}"
  aws cloudwatch describe-alarms --alarm-name-prefix "$PREFIX" --state-value ALARM \
    --profile "$PROFILE" --region "$REGION" \
    --query "MetricAlarms[].{Name:AlarmName,Desc:AlarmDescription}" --output table 2>/dev/null
else
  echo -e "  ${GREEN}🔕 No active alarms${RESET}"
fi

echo ""
