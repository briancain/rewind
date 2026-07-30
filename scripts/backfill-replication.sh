#!/bin/bash
# scripts/backfill-replication.sh — one-time S3 Batch Replication backfill.
#
# Cross-Region Replication only replicates objects written AFTER the replication rule is enabled.
# Objects that predate it must be seeded once via an S3 Batch Replication job (operation
# S3ReplicateObject) — this is a one-time DATA operation, so it lives as a script (like reindex.sh),
# not in Terraform (which owns steady-state infra). The Batch Operations IAM role it needs is transient
# tooling created here on demand.
#
# Idempotent: re-running skips source buckets with no un-replicated objects, and reuses the role.
#
# Usage:
#   ./scripts/backfill-replication.sh                  # all replicated buckets, both regions
#   REGIONS="us-west-2 us-east-2" ./scripts/backfill-replication.sh
set -euo pipefail

PROFILE="${PROFILE:-rewind}"
NAME="${NAME:-rewind-dev}"
REGIONS="${REGIONS:-us-west-2 us-east-2}"
BUCKETS="${BUCKETS:-videos}"
ROLE_NAME="${NAME}-s3-batch-replication"

ACCOUNT=$(aws sts get-caller-identity --profile "$PROFILE" --query Account --output text)
echo "▶ Account: $ACCOUNT | role: $ROLE_NAME | regions: $REGIONS"

# --- Ensure the S3 Batch Operations role (trusted by batchoperations.s3.amazonaws.com). -----------
# Scoped to every replicated source bucket so one role serves all jobs. InitiateReplication is the
# key permission; the actual cross-region copy still uses each bucket's own live replication role.
ensure_role() {
  if aws iam get-role --role-name "$ROLE_NAME" --profile "$PROFILE" >/dev/null 2>&1; then
    echo "  role exists"
  else
    echo "  creating role $ROLE_NAME"
    aws iam create-role --role-name "$ROLE_NAME" --profile "$PROFILE" \
      --assume-role-policy-document '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"batchoperations.s3.amazonaws.com"},"Action":"sts:AssumeRole"}]}' >/dev/null
  fi

  # Build the resource list across all replicated buckets/regions.
  local bkt_arns="" obj_arns=""
  for r in $REGIONS; do for b in $BUCKETS; do
    bkt_arns="${bkt_arns}\"arn:aws:s3:::${NAME}-${b}-${r}\","
    obj_arns="${obj_arns}\"arn:aws:s3:::${NAME}-${b}-${r}/*\","
    bkt_arns="${bkt_arns}\"arn:aws:s3:::${NAME}-raw-${r}\","
    obj_arns="${obj_arns}\"arn:aws:s3:::${NAME}-raw-${r}/*\","
  done; done
  bkt_arns="${bkt_arns%,}"; obj_arns="${obj_arns%,}"

  aws iam put-role-policy --role-name "$ROLE_NAME" --policy-name s3-batch-replication --profile "$PROFILE" \
    --policy-document "{\"Version\":\"2012-10-17\",\"Statement\":[
      {\"Effect\":\"Allow\",\"Action\":[\"s3:InitiateReplication\"],\"Resource\":[${obj_arns}]},
      {\"Effect\":\"Allow\",\"Action\":[\"s3:GetReplicationConfiguration\",\"s3:ListBucket\",\"s3:GetBucketLocation\",\"s3:PutInventoryConfiguration\"],\"Resource\":[${bkt_arns}]},
      {\"Effect\":\"Allow\",\"Action\":[\"s3:GetObjectVersion\",\"s3:GetObjectVersionAcl\",\"s3:GetObjectVersionTagging\",\"s3:GetObjectVersionForReplication\"],\"Resource\":[${obj_arns}]},
      {\"Effect\":\"Allow\",\"Action\":[\"s3:PutObject\"],\"Resource\":[${obj_arns}]}
    ]}" >/dev/null
  echo "  role policy updated"
}

ensure_role
ROLE_ARN="arn:aws:iam::${ACCOUNT}:role/${ROLE_NAME}"
echo "  waiting 15s for IAM propagation..."; sleep 15

# --- Create one Batch Replication job per source bucket that has un-replicated objects. ------------
JOBS=()
for r in $REGIONS; do
  for b in $BUCKETS; do
    src="${NAME}-${b}-${r}"
    # Skip buckets with no replication rule or no objects.
    if ! aws s3api get-bucket-replication --bucket "$src" --region "$r" --profile "$PROFILE" >/dev/null 2>&1; then
      echo "▶ $src: no replication config — skip"; continue
    fi
    n=$(aws s3 ls "s3://$src" --recursive --summarize --profile "$PROFILE" 2>/dev/null | awk '/Total Objects:/{print $3}')
    if [ "${n:-0}" -eq 0 ]; then
      echo "▶ $src: 0 objects — skip (live replication covers new writes)"; continue
    fi
    echo "▶ $src ($n objects): creating S3 Batch Replication job"
    JOB_ID=$(aws s3control create-job \
      --account-id "$ACCOUNT" --region "$r" --profile "$PROFILE" \
      --priority 10 --role-arn "$ROLE_ARN" --no-confirmation-required \
      --operation '{"S3ReplicateObject":{}}' \
      --manifest-generator "{\"S3JobManifestGenerator\":{\"ExpectedBucketOwner\":\"${ACCOUNT}\",\"SourceBucket\":\"arn:aws:s3:::${src}\",\"EnableManifestOutput\":false,\"Filter\":{\"EligibleForReplication\":true,\"ObjectReplicationStatuses\":[\"NONE\",\"FAILED\"]}}}" \
      --report "{\"Bucket\":\"arn:aws:s3:::${NAME}-raw-${r}\",\"Prefix\":\"batch-replication-reports\",\"Format\":\"Report_CSV_20180820\",\"Enabled\":true,\"ReportScope\":\"AllTasks\"}" \
      --query JobId --output text)
    echo "  job: $JOB_ID (region $r)"
    JOBS+=("$r:$JOB_ID")
  done
done

if [ ${#JOBS[@]} -eq 0 ]; then
  echo "✓ Nothing to backfill."
  exit 0
fi

echo ""
echo "✓ Submitted ${#JOBS[@]} job(s). Track with:"
for j in "${JOBS[@]}"; do
  echo "  aws s3control describe-job --account-id $ACCOUNT --region ${j%%:*} --job-id ${j##*:} --profile $PROFILE --query 'Job.{status:Status,done:ProgressSummary}'"
done
