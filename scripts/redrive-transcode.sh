#!/bin/bash
# scripts/redrive-transcode.sh — manually re-drive a stranded transcode.
#
# The reconcile CronJob is DETECT + ALARM only: when the `transcode-stuck-processing` alarm fires for
# a video stuck in `status=processing` (a lost MediaConvert completion event, or a region failure),
# an operator re-drives it with this script. It re-enqueues the original transcode job to the
# region's transcode SQS queue; the transcode consumer re-submits MediaConvert from the raw bucket,
# and the conditional-publish guard (status <> deleted) keeps the re-drive safe.
#
# This is the deliberate stopgap while automated, capped re-drive is deferred. Run it from
# the region that owns the raw upload (raw is regional) — pass REGION to target us-east-2.
#
# Usage:  ./scripts/redrive-transcode.sh <video_id>
#         REGION=us-east-2 ./scripts/redrive-transcode.sh <video_id>
set -euo pipefail

VIDEO_ID="${1:-}"
if [ -z "$VIDEO_ID" ]; then
  echo "Usage: $0 <video_id>" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REGION="${REGION:-us-west-2}"
TF_DIR="$ROOT/infra/environments/dev/${REGION}"
PROFILE="rewind"

if [ ! -d "$TF_DIR" ]; then
  echo "✗ No environment directory for region '$REGION' (expected $TF_DIR)" >&2
  exit 1
fi

echo "▶ Reading Terraform outputs ($REGION)..."
TF_OUT=$(cd "$TF_DIR" && terraform output -json)
tf() { echo "$TF_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)$1)"; }

REGION=$(tf "['region']['value']")
SQS_URL=$(tf "['sqs_queue_url']['value']")
BUCKET_RAW=$(tf "['s3_bucket_raw']['value']")

AWS="aws --region $REGION --profile $PROFILE"

# Confirm the video is actually stuck in `processing` before re-driving (avoid resurrecting a
# published/deleted video). The conditional-publish guard would reject a deleted one anyway.
TABLE_PREFIX=$(tf "['dynamodb_table_prefix']['value']")
STATUS=$($AWS dynamodb get-item \
  --table-name "${TABLE_PREFIX}videos" \
  --key "{\"video_id\":{\"S\":\"${VIDEO_ID}\"}}" \
  --query 'Item.status.S' --output text 2>/dev/null || echo "")

if [ "$STATUS" != "processing" ]; then
  echo "⚠️  Video ${VIDEO_ID} has status '${STATUS:-<not found>}', not 'processing'." >&2
  echo "    Re-drive aborted (only stranded processing videos should be re-driven)." >&2
  exit 1
fi

# Find the raw object for this video (the upload key under raw/{video_id}/).
echo "▶ Locating raw object under s3://${BUCKET_RAW}/raw/${VIDEO_ID}/ ..."
S3_KEY=$($AWS s3api list-objects-v2 \
  --bucket "$BUCKET_RAW" \
  --prefix "raw/${VIDEO_ID}/" \
  --query 'Contents[0].Key' --output text 2>/dev/null || echo "None")

if [ "$S3_KEY" = "None" ] || [ -z "$S3_KEY" ]; then
  echo "✗ No raw object found under raw/${VIDEO_ID}/ in ${BUCKET_RAW}." >&2
  echo "  If this region doesn't own the raw upload, run from the owning region, or have the" >&2
  echo "  owner re-upload (raw is regional and not replicated)." >&2
  exit 1
fi

MSG=$(python3 -c "import json,sys; print(json.dumps({'video_id': sys.argv[1], 's3_key': sys.argv[2], 'bucket': sys.argv[3]}))" \
  "$VIDEO_ID" "$S3_KEY" "$BUCKET_RAW")

echo "▶ Re-enqueueing transcode job:"
echo "    $MSG"
$AWS sqs send-message --queue-url "$SQS_URL" --message-body "$MSG" >/dev/null

echo "✓ Re-drive enqueued for ${VIDEO_ID}. Watch transcode logs + the videos row flip to published."
