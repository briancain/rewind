#!/bin/bash
# scripts/reindex.sh — Seed/rebuild the OpenSearch index from the videos table (source of truth).
#
# Runs as an in-cluster Kubernetes Job using the `search` ServiceAccount's IRSA role (which already
# has DynamoDB Scan + OpenSearch access) — NOT a public HTTP endpoint and no shared secret. The Job
# runs the same deployed search image as `service reindex`, which scans the videos table and
# reconciles the index (upsert public+published, remove the rest). Idempotent; safe to re-run.
#
# Use it to: seed a freshly deployed region, recover a lost index, or reindex after a mapping change.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Region selected via the REGION env var (default us-west-2), same pattern as deploy.sh:
#   REGION=us-east-2 ./scripts/reindex.sh
REGION="${REGION:-us-west-2}"
TF_DIR="$ROOT/infra/environments/dev/${REGION}"
PROFILE="rewind"
NAMESPACE="rewind"

echo "▶ Reading Terraform outputs..."
TF_OUT=$(cd "$TF_DIR" && terraform output -json)
tf() { echo "$TF_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)$1)"; }

REGION=$(tf "['region']['value']")
CLUSTER=$(tf "['cluster_name']['value']")
TABLE_PREFIX=$(tf "['dynamodb_table_prefix']['value']")
OPENSEARCH=$(tf "['opensearch_endpoint']['value']")

echo "▶ Updating kubeconfig..."
aws eks update-kubeconfig --name "$CLUSTER" --region "$REGION" --profile "$PROFILE" >/dev/null 2>&1

# Use the exact image the running search Deployment uses, so the Job matches what's deployed.
IMAGE=$(kubectl get deployment search -n "$NAMESPACE" \
  -o jsonpath='{.spec.template.spec.containers[0].image}')
if [ -z "$IMAGE" ]; then
  echo "✗ Could not resolve the deployed search image (is the search Deployment running?)" >&2
  exit 1
fi

JOB="reindex-search-$(date +%s)"
echo "▶ Launching Job $JOB (image: $IMAGE)..."

kubectl apply -n "$NAMESPACE" -f - <<EOF
apiVersion: batch/v1
kind: Job
metadata:
  name: $JOB
  namespace: $NAMESPACE
  labels:
    app: search
    role: reindex
spec:
  backoffLimit: 2
  ttlSecondsAfterFinished: 600
  template:
    metadata:
      labels:
        app: search
        role: reindex
    spec:
      serviceAccountName: search
      restartPolicy: Never
      containers:
        - name: reindex
          image: $IMAGE
          command: ["service", "reindex"]
          env:
            - name: OPENSEARCH_ENDPOINT
              value: "https://$OPENSEARCH"
            - name: TABLE_PREFIX
              value: "$TABLE_PREFIX"
            - name: AWS_DEFAULT_REGION
              value: "$REGION"
EOF

echo "▶ Waiting for completion (timeout 5m)..."
if kubectl wait --for=condition=complete --timeout=300s "job/$JOB" -n "$NAMESPACE" 2>/dev/null; then
  echo "✓ Reindex complete"
else
  echo "⚠️  Job did not report complete within timeout (it may still be running or failed)" >&2
fi

echo "▶ Logs:"
kubectl logs -n "$NAMESPACE" "job/$JOB" || true

echo ""
echo "  (Job $JOB auto-cleans after 10 minutes via ttlSecondsAfterFinished.)"
