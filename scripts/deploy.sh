#!/bin/bash
# scripts/deploy.sh — Build, push to ECR, and deploy services to EKS
set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Region is selected via the REGION env var (default us-west-2) so the same script deploys to either
# region's cluster: `REGION=us-east-2 ./scripts/deploy.sh [TAG]`. TAG stays the first positional arg.
REGION="${REGION:-us-west-2}"
TF_DIR="$ROOT/infra/environments/dev/${REGION}"
TAG="${1:-$(git -C "$ROOT" rev-parse --short HEAD)}"
PROFILE="rewind"

if [ ! -d "$TF_DIR" ]; then
  echo "✗ No environment directory for region '$REGION' (expected $TF_DIR)" >&2
  exit 1
fi
echo "▶ Target region: $REGION  ($TF_DIR)"

SERVICES=(identity video-catalog upload transcode streaming social search delete-cleanup)

echo "▶ Reading Terraform outputs..."
TF_OUT=$(cd "$TF_DIR" && terraform output -json)

tf() { echo "$TF_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)$1)"; }

REGION=$(tf "['region']['value']")
CLUSTER=$(tf "['cluster_name']['value']")
SQS_URL=$(tf "['sqs_queue_url']['value']")
SEARCH_QUEUE_URL=$(tf "['search_index_queue_url']['value']")
OPENSEARCH=$(tf "['opensearch_endpoint']['value']")
TABLE_PREFIX=$(tf "['dynamodb_table_prefix']['value']")
BUCKET_RAW=$(tf "['s3_bucket_raw']['value']")
BUCKET_VIDEOS=$(tf "['s3_bucket_videos']['value']")
MEDIACONVERT_ROLE=$(tf "['mediaconvert_role_arn']['value']")
COMPLETION_QUEUE_URL=$(tf "['transcode_completions_queue_url']['value']")
CLEANUP_QUEUE_URL=$(tf "['delete_cleanup_queue_url']['value']")
REGISTRY=$(tf "['ecr_repository_urls']['value']['identity']" | sed 's|/[^/]*$||')
DOMAIN=$(tf "['domain']['value']")

# Content-CDN distribution id, read from the separate infra/cdn stack (a single global distribution).
# deploy.sh runs BEFORE the cdn stack on a fresh bootstrap, and the env can't read the cdn stack's
# state without an env<->cdn dependency cycle — so read it here directly, tolerating absence: empty
# until the cdn stack is applied, after which a re-deploy wires it into the delete-cleanup worker
# (which skips edge invalidation while it's empty). The id is global (region-independent).
CDN_DISTRIBUTION_ID=$( (cd "$ROOT/infra/cdn" && terraform output -raw distribution_id 2>/dev/null) || true )
if [ -z "$CDN_DISTRIBUTION_ID" ]; then
  echo "  ⚠ infra/cdn distribution_id not available — delete-cleanup CDN invalidation will be disabled until the cdn stack is applied and you re-deploy"
fi

echo "  Region: $REGION | Cluster: $CLUSTER | Tag: $TAG"

echo "▶ Logging in to ECR..."
aws ecr get-login-password --region "$REGION" --profile "$PROFILE" | \
  finch login --username AWS --password-stdin "$REGISTRY"

echo "▶ Building and pushing images..."
for svc in "${SERVICES[@]}"; do
  REPO=$(tf "['ecr_repository_urls']['value']['$svc']")
  echo "  → $svc"
  finch build \
    --build-arg SERVICE_NAME="$svc" \
    -t "${REPO}:${TAG}" \
    -f "$ROOT/docker/Dockerfile" \
    "$ROOT"
  finch push "${REPO}:${TAG}"
done

echo "▶ Updating kubeconfig..."
aws eks update-kubeconfig --name "$CLUSTER" --region "$REGION" --profile "$PROFILE" 2>/dev/null

role_arn() { tf "['service_role_arns']['value']['$1']"; }

echo "▶ Deploying services via Helm..."
for svc in "${SERVICES[@]}"; do
  REPO=$(tf "['ecr_repository_urls']['value']['$svc']")
  ROLE=$(role_arn "$svc")

  # Build common --set args
  SETS=(
    --set image.repository="$REPO"
    --set image.tag="$TAG"
    --set serviceAccount.roleArn="$ROLE"
  )

  # Per-service env vars
  case $svc in
    identity)
      SETS+=(
        --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}"
        --set "env[1].name=AWS_DEFAULT_REGION,env[1].value=${REGION}"
        --set "env[2].name=DISABLE_SES,env[2].value=1"
        --set "env[3].name=FROM_EMAIL,env[3].value=noreply@${DOMAIN}"
      ) ;;
    video-catalog)
      SETS+=(
        --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}"
        --set "env[1].name=AWS_DEFAULT_REGION,env[1].value=${REGION}"
      ) ;;
    upload)
      SETS+=(
        --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}"
        --set "env[1].name=S3_BUCKET,env[1].value=${BUCKET_RAW}"
        --set "env[2].name=SQS_QUEUE_URL,env[2].value=${SQS_URL}"
        --set "env[3].name=AWS_DEFAULT_REGION,env[3].value=${REGION}"
      ) ;;
    transcode)
      SETS+=(
        --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}"
        --set "env[1].name=S3_BUCKET_RAW,env[1].value=${BUCKET_RAW}"
        --set "env[2].name=OUTPUT_BUCKET,env[2].value=${BUCKET_VIDEOS}"
        --set "env[3].name=SQS_QUEUE_URL,env[3].value=${SQS_URL}"
        --set "env[4].name=AWS_DEFAULT_REGION,env[4].value=${REGION}"
        --set "env[5].name=CDN_BASE_URL,env[5].value=https://cdn.${DOMAIN}"
        --set "env[6].name=MEDIACONVERT_ROLE,env[6].value=${MEDIACONVERT_ROLE}"
        --set "env[7].name=COMPLETION_QUEUE_URL,env[7].value=${COMPLETION_QUEUE_URL}"
      )
      # MediaConvert is active: transcode submits HLS jobs and the completion consumer publishes the
      # video on the EventBridge COMPLETE event. To fall back to the ffmpeg path (e.g.
      # for a fast rollback), add: --set "env[9].name=DISABLE_MEDIACONVERT,env[9].value=1".
      # NOTE: no SEARCH_ENDPOINT here — in the cloud, OpenSearch is kept in sync by the videos
      # DynamoDB stream -> EventBridge Pipe -> SQS -> search consumer pipeline.
      # The transcode->search /index shim is retained only for local dev (see scripts/dev.sh).
      ;;
    streaming)
      SETS+=(
        --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}"
        --set "env[1].name=VIDEO_BUCKET,env[1].value=${BUCKET_VIDEOS}"
        --set "env[2].name=AWS_DEFAULT_REGION,env[2].value=${REGION}"
      ) ;;
    social)
      SETS+=(
        --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}"
        --set "env[1].name=AWS_DEFAULT_REGION,env[1].value=${REGION}"
      ) ;;
    search)
      SETS+=(
        --set "env[0].name=OPENSEARCH_ENDPOINT,env[0].value=https://${OPENSEARCH}"
        --set "env[1].name=AWS_DEFAULT_REGION,env[1].value=${REGION}"
        --set "env[2].name=STREAM_QUEUE_URL,env[2].value=${SEARCH_QUEUE_URL}"
      )
      # Index backfill/seeding is a one-off Kubernetes Job (scripts/reindex.sh), not an HTTP
      # endpoint — it runs `service reindex` under the search ServiceAccount's IRSA role.
      ;;
    delete-cleanup)
      # Cascade cleanup worker: drains the delete-cleanup FIFO queue fed by the videos-stream
      # Pipe (filtered to soft-deletes) and reclaims the deleted video's rows + S3 objects. The
      # consumer only runs because CLEANUP_QUEUE_URL is set (health-only otherwise).
      SETS+=(
        --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}"
        --set "env[1].name=AWS_DEFAULT_REGION,env[1].value=${REGION}"
        --set "env[2].name=CLEANUP_QUEUE_URL,env[2].value=${CLEANUP_QUEUE_URL}"
        --set "env[3].name=VIDEO_BUCKET,env[3].value=${BUCKET_VIDEOS}"
        --set "env[4].name=RAW_BUCKET,env[4].value=${BUCKET_RAW}"
      )
      # Edge invalidation on cascade delete — only when the cdn stack's distribution id is available
      # (see the CDN_DISTRIBUTION_ID read above). The worker skips invalidation when it's unset.
      if [ -n "$CDN_DISTRIBUTION_ID" ]; then
        SETS+=(
          --set "env[5].name=CDN_DISTRIBUTION_ID,env[5].value=${CDN_DISTRIBUTION_ID}"
        )
      fi
      ;;
  esac

  echo "  → $svc"
  helm upgrade --install "$svc" "$ROOT/helm/rewind-service" \
    -f "$ROOT/helm/values/${svc}.yaml" \
    --namespace rewind \
    "${SETS[@]}" \
    --wait --timeout 300s
done

# --- Transcode reconcile CronJob ---
# Reuses the transcode image (built in the loop above) as a per-region CronJob (`transcode
# reconcile`) that detects + alarms on stuck `processing` videos. A separate release off the shared
# chart in CronJob mode (cronjob.enabled in helm/values/transcode-reconcile.yaml), under its own
# read-only IRSA role. No --wait: a CronJob has no pods to become ready. Tune the cadence/threshold
# in the values file / the RECONCILE_STUCK_THRESHOLD_MINS env below.
echo "  → transcode-reconcile (CronJob)"
TRANSCODE_REPO=$(tf "['ecr_repository_urls']['value']['transcode']")
helm upgrade --install transcode-reconcile "$ROOT/helm/rewind-service" \
  -f "$ROOT/helm/values/transcode-reconcile.yaml" \
  --namespace rewind \
  --set image.repository="$TRANSCODE_REPO" \
  --set image.tag="$TAG" \
  --set serviceAccount.roleArn="$(role_arn transcode-reconcile)" \
  --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}" \
  --set "env[1].name=AWS_DEFAULT_REGION,env[1].value=${REGION}" \
  --set "env[2].name=RECONCILE_STUCK_THRESHOLD_MINS,env[2].value=60"

# --- Delete-cleanup reconcile CronJob ---
# Reuses the delete-cleanup image (built in the loop above) as a per-region CronJob (`delete-cleanup
# reconcile`) that detects + alarms on `deleted` tombstones whose dependent data was never reclaimed
# (a failed videos-to-cleanup Pipe or a partial cleanup — neither caught by the cleanup DLQ alarm). A
# separate release off the shared chart in CronJob mode (cronjob.enabled in
# helm/values/delete-cleanup-reconcile.yaml), under its own read-only IRSA role. It probes the
# dependent stores, so it needs the bucket names; no CLEANUP_QUEUE_URL (it never touches the queue).
# No --wait: a CronJob has no pods to become ready. Tune cadence/threshold in the values file / below.
echo "  → delete-cleanup-reconcile (CronJob)"
CLEANUP_REPO=$(tf "['ecr_repository_urls']['value']['delete-cleanup']")
helm upgrade --install delete-cleanup-reconcile "$ROOT/helm/rewind-service" \
  -f "$ROOT/helm/values/delete-cleanup-reconcile.yaml" \
  --namespace rewind \
  --set image.repository="$CLEANUP_REPO" \
  --set image.tag="$TAG" \
  --set serviceAccount.roleArn="$(role_arn delete-cleanup-reconcile)" \
  --set "env[0].name=TABLE_PREFIX,env[0].value=${TABLE_PREFIX}" \
  --set "env[1].name=AWS_DEFAULT_REGION,env[1].value=${REGION}" \
  --set "env[2].name=VIDEO_BUCKET,env[2].value=${BUCKET_VIDEOS}" \
  --set "env[3].name=RAW_BUCKET,env[3].value=${BUCKET_RAW}" \
  --set "env[4].name=DELETION_RECONCILE_THRESHOLD_MINS,env[4].value=30"

# --- Frontend ---
echo "▶ Building and pushing frontend..."
FRONTEND_REPO=$(tf "['ecr_repository_urls']['value']['frontend']")
finch build \
  --build-arg "NEXT_PUBLIC_IDENTITY_URL=https://identity.${DOMAIN}" \
  --build-arg "NEXT_PUBLIC_CATALOG_URL=https://catalog.${DOMAIN}" \
  --build-arg "NEXT_PUBLIC_UPLOAD_URL=https://upload.${DOMAIN}" \
  --build-arg "NEXT_PUBLIC_STREAMING_URL=https://streaming.${DOMAIN}" \
  --build-arg "NEXT_PUBLIC_SOCIAL_URL=https://social.${DOMAIN}" \
  --build-arg "NEXT_PUBLIC_SEARCH_URL=https://search.${DOMAIN}" \
  --build-arg "NEXT_PUBLIC_SITE_URL=https://${DOMAIN}" \
  --build-arg "NEXT_PUBLIC_CDN_URL=https://cdn.${DOMAIN}" \
  -t "${FRONTEND_REPO}:${TAG}" \
  -f "$ROOT/docker/Dockerfile.frontend" \
  "$ROOT"
finch push "${FRONTEND_REPO}:${TAG}"

echo "  → frontend"
helm upgrade --install frontend "$ROOT/helm/rewind-service" \
  -f "$ROOT/helm/values/frontend.yaml" \
  --namespace rewind \
  --set image.repository="$FRONTEND_REPO" \
  --set image.tag="$TAG" \
  --wait --timeout 300s

# --- Canary ---
# Its own image + its own standalone chart (helm/canary): the scheduled CronJobs (shallow + deep)
# are unrelated to the long-running service Deployments, so they don't touch the shared chart.
# Owner/viewer credentials are NOT set here — they live in the `canary-credentials` Secret created
# by `scripts/canary.sh setup` (in-cluster only, never in git/TF state).
echo "▶ Building and pushing canary..."
CANARY_REPO=$(tf "['ecr_repository_urls']['value']['canary']")
finch build \
  --build-arg SERVICE_NAME=canary \
  -t "${CANARY_REPO}:${TAG}" \
  -f "$ROOT/docker/Dockerfile" \
  "$ROOT"
finch push "${CANARY_REPO}:${TAG}"

echo "  → canary (CronJobs)"
helm upgrade --install canary "$ROOT/helm/canary" \
  --namespace rewind \
  --set image.repository="$CANARY_REPO" \
  --set image.tag="$TAG" \
  --set serviceAccount.roleArn="$(role_arn canary)" \
  --set config.domain="$DOMAIN" \
  --set config.region="$REGION" \
  --set config.cdnBase="https://cdn.${DOMAIN}" \
  --set config.tablePrefix="$TABLE_PREFIX" \
  --set config.videoBucket="$BUCKET_VIDEOS" \
  --set config.rawBucket="$BUCKET_RAW"

echo ""
echo "✓ Deploy complete!"
kubectl get pods -n rewind
