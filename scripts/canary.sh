#!/bin/bash
# scripts/canary.sh — Operate the cloud integration canary.
#
# The canary runs in-cluster as scheduled CronJobs (canary-shallow, canary-deep) deployed by
# deploy.sh via helm/canary. This script handles the one-time setup and on-demand runs:
#
#   ./scripts/canary.sh setup     Create the canary-credentials Secret (generates owner/viewer
#                                 passwords if absent) and register those accounts via a one-off
#                                 `service setup` Job. Run once after the first deploy.
#   ./scripts/canary.sh shallow   Trigger an on-demand shallow run now (from the CronJob).
#   ./scripts/canary.sh deep      Trigger an on-demand deep run now (from the CronJob).
#   ./scripts/canary.sh enable    Un-suspend the scheduled CronJobs (start running on schedule).
#   ./scripts/canary.sh disable   Suspend the scheduled CronJobs (stop scheduled runs).
#
# CronJobs deploy SUSPENDED (helm/canary values: suspend=true), so nothing runs on a schedule until
# `enable` is called. On-demand runs work regardless of suspend state.
# Credentials live ONLY in the in-cluster `canary-credentials` Secret — never in git or TF state.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Region selected via the REGION env var (default us-west-2), same pattern as deploy.sh:
#   REGION=us-east-2 ./scripts/canary.sh setup
REGION="${REGION:-us-west-2}"
TF_DIR="$ROOT/infra/environments/dev/${REGION}"
PROFILE="rewind"
NAMESPACE="rewind"
SECRET="canary-credentials"

usage() { echo "Usage: ./scripts/canary.sh {setup|shallow|deep|enable|disable}"; exit 1; }

[ $# -ge 1 ] || usage
CMD="$1"

echo "▶ Reading Terraform outputs..."
TF_OUT=$(cd "$TF_DIR" && terraform output -json)
tf() { echo "$TF_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)$1)"; }

REGION=$(tf "['region']['value']")
CLUSTER=$(tf "['cluster_name']['value']")
DOMAIN=$(tf "['domain']['value']")
TABLE_PREFIX=$(tf "['dynamodb_table_prefix']['value']")

echo "▶ Updating kubeconfig..."
aws eks update-kubeconfig --name "$CLUSTER" --region "$REGION" --profile "$PROFILE" >/dev/null 2>&1

# Resolve the deployed canary image from the CronJob so on-demand/setup Jobs match what's scheduled.
canary_image() {
  kubectl get cronjob canary-deep -n "$NAMESPACE" \
    -o jsonpath='{.spec.jobTemplate.spec.template.spec.containers[0].image}' 2>/dev/null
}

ensure_secret() {
  if kubectl get secret "$SECRET" -n "$NAMESPACE" >/dev/null 2>&1; then
    echo "  ✓ Secret $SECRET already exists (leaving as-is)"
    return
  fi
  echo "  Creating Secret $SECRET with generated passwords..."
  # Region-scoped accounts: the users table is a Global Table (shared across regions), so each region's
  # canary owns a DISTINCT identity (canary-{owner,viewer}-${REGION}@...). This avoids registration
  # collisions and keeps each region's deep (mutating) run from racing on another region's data when
  # the CronJobs run in every region. @canary.invalid is non-routable; SES is disabled.
  kubectl create secret generic "$SECRET" -n "$NAMESPACE" \
    --from-literal=CANARY_OWNER_EMAIL="canary-owner-${REGION}@canary.invalid" \
    --from-literal=CANARY_OWNER_PASSWORD="$(openssl rand -hex 24)" \
    --from-literal=CANARY_VIEWER_EMAIL="canary-viewer-${REGION}@canary.invalid" \
    --from-literal=CANARY_VIEWER_PASSWORD="$(openssl rand -hex 24)"
  echo "  ✓ Secret created"
}

run_setup_job() {
  local image job
  image="$(canary_image)"
  if [ -z "$image" ]; then
    echo "✗ Could not resolve the canary image (is the canary chart deployed? run deploy.sh)" >&2
    exit 1
  fi
  job="canary-setup-$(date +%s)"
  echo "▶ Launching setup Job $job (image: $image)..."
  kubectl apply -n "$NAMESPACE" -f - <<EOF
apiVersion: batch/v1
kind: Job
metadata:
  name: $job
  namespace: $NAMESPACE
  labels:
    app: canary
    role: setup
spec:
  backoffLimit: 1
  ttlSecondsAfterFinished: 600
  template:
    metadata:
      labels:
        app: canary
        role: setup
    spec:
      serviceAccountName: canary
      restartPolicy: Never
      nodeSelector:
        kubernetes.io/arch: arm64
      containers:
        - name: canary
          image: $image
          command: ["service", "setup"]
          envFrom:
            - secretRef:
                name: $SECRET
          env:
            - name: CANARY_DOMAIN
              value: "$DOMAIN"
            - name: CANARY_REGION
              value: "$REGION"
            - name: AWS_DEFAULT_REGION
              value: "$REGION"
            - name: TABLE_PREFIX
              value: "$TABLE_PREFIX"
EOF
  echo "▶ Waiting for completion (timeout 3m)..."
  kubectl wait --for=condition=complete --timeout=180s "job/$job" -n "$NAMESPACE" 2>/dev/null \
    && echo "✓ Setup complete" \
    || echo "⚠️  Setup Job did not report complete (check logs)" >&2
  echo "▶ Logs:"; kubectl logs -n "$NAMESPACE" "job/$job" || true
}

trigger_run() {
  local tier="$1" job
  job="canary-${tier}-$(date +%s)"
  echo "▶ Triggering on-demand $tier run as $job..."
  kubectl create job "$job" --from="cronjob/canary-${tier}" -n "$NAMESPACE"
  echo "▶ Waiting for completion (timeout 5m)..."
  kubectl wait --for=condition=complete --timeout=300s "job/$job" -n "$NAMESPACE" 2>/dev/null \
    && echo "✓ $tier run PASSED" \
    || echo "⚠️  $tier run did not complete successfully (check logs below)" >&2
  echo "▶ Logs:"; kubectl logs -n "$NAMESPACE" "job/$job" || true
}

set_suspend() {
  local value="$1" verb="$2"
  for tier in shallow deep; do
    kubectl patch cronjob "canary-${tier}" -n "$NAMESPACE" \
      -p "{\"spec\":{\"suspend\":${value}}}"
  done
  echo "✓ Scheduled canaries ${verb}"
}

case "$CMD" in
  setup)
    ensure_secret
    run_setup_job
    ;;
  shallow|deep)
    trigger_run "$CMD"
    ;;
  enable)
    set_suspend false "ENABLED (running on schedule)"
    ;;
  disable)
    set_suspend true "DISABLED (scheduled runs suspended)"
    ;;
  *)
    usage
    ;;
esac
