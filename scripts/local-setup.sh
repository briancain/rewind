#!/bin/bash
# scripts/local-setup.sh — Creates all DynamoDB tables, S3 buckets, and SQS queues for local dev
set -e

DDB="aws dynamodb --endpoint-url http://localhost:8000 --region us-west-2"
S3="aws --endpoint-url http://localhost:4566 --region us-west-2"
SQS="aws sqs --endpoint-url http://localhost:4566 --region us-west-2"

echo "Creating DynamoDB tables..."

$DDB create-table --table-name users \
  --attribute-definitions AttributeName=user_id,AttributeType=S AttributeName=email,AttributeType=S \
  --key-schema AttributeName=user_id,KeyType=HASH \
  --global-secondary-indexes '[{"IndexName":"email-index","KeySchema":[{"AttributeName":"email","KeyType":"HASH"}],"Projection":{"ProjectionType":"ALL"},"ProvisionedThroughput":{"ReadCapacityUnits":5,"WriteCapacityUnits":5}}]' \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  users ✓" || echo "  users (exists)"

$DDB create-table --table-name sessions \
  --attribute-definitions AttributeName=session_token,AttributeType=S AttributeName=user_id,AttributeType=S \
  --key-schema AttributeName=session_token,KeyType=HASH \
  --global-secondary-indexes '[{"IndexName":"user-id-index","KeySchema":[{"AttributeName":"user_id","KeyType":"HASH"}],"Projection":{"ProjectionType":"KEYS_ONLY"},"ProvisionedThroughput":{"ReadCapacityUnits":5,"WriteCapacityUnits":5}}]' \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  sessions ✓" || echo "  sessions (exists)"

$DDB create-table --table-name verification_tokens \
  --attribute-definitions AttributeName=token,AttributeType=S \
  --key-schema AttributeName=token,KeyType=HASH \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  verification_tokens ✓" || echo "  verification_tokens (exists)"

$DDB create-table --table-name videos \
  --attribute-definitions AttributeName=video_id,AttributeType=S AttributeName=status,AttributeType=S AttributeName=channel_id,AttributeType=S \
  --key-schema AttributeName=video_id,KeyType=HASH \
  --global-secondary-indexes '[{"IndexName":"status-index","KeySchema":[{"AttributeName":"status","KeyType":"HASH"}],"Projection":{"ProjectionType":"ALL"},"ProvisionedThroughput":{"ReadCapacityUnits":5,"WriteCapacityUnits":5}},{"IndexName":"channel-index","KeySchema":[{"AttributeName":"channel_id","KeyType":"HASH"}],"Projection":{"ProjectionType":"ALL"},"ProvisionedThroughput":{"ReadCapacityUnits":5,"WriteCapacityUnits":5}}]' \
  --stream-specification StreamEnabled=true,StreamViewType=NEW_AND_OLD_IMAGES \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  videos ✓" || echo "  videos (exists)"

$DDB create-table --table-name reactions \
  --attribute-definitions AttributeName=video_id,AttributeType=S AttributeName=user_id,AttributeType=S \
  --key-schema AttributeName=video_id,KeyType=HASH AttributeName=user_id,KeyType=RANGE \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  reactions ✓" || echo "  reactions (exists)"

$DDB create-table --table-name comments \
  --attribute-definitions AttributeName=video_id,AttributeType=S AttributeName=comment_id,AttributeType=S \
  --key-schema AttributeName=video_id,KeyType=HASH AttributeName=comment_id,KeyType=RANGE \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  comments ✓" || echo "  comments (exists)"

$DDB create-table --table-name video_stats \
  --attribute-definitions AttributeName=video_id,AttributeType=S \
  --key-schema AttributeName=video_id,KeyType=HASH \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  video_stats ✓" || echo "  video_stats (exists)"

$DDB create-table --table-name invite_codes \
  --attribute-definitions AttributeName=code,AttributeType=S \
  --key-schema AttributeName=code,KeyType=HASH \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  invite_codes ✓" || echo "  invite_codes (exists)"

$DDB create-table --table-name comment_reactions \
  --attribute-definitions AttributeName=video_id,AttributeType=S AttributeName=sk,AttributeType=S \
  --key-schema AttributeName=video_id,KeyType=HASH AttributeName=sk,KeyType=RANGE \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  comment_reactions ✓" || echo "  comment_reactions (exists)"

$DDB create-table --table-name view_history \
  --attribute-definitions AttributeName=user_id,AttributeType=S AttributeName=watched_at,AttributeType=S AttributeName=video_id,AttributeType=S \
  --key-schema AttributeName=user_id,KeyType=HASH AttributeName=watched_at,KeyType=RANGE \
  --global-secondary-indexes '[{"IndexName":"video-id-index","KeySchema":[{"AttributeName":"video_id","KeyType":"HASH"}],"Projection":{"ProjectionType":"KEYS_ONLY"},"ProvisionedThroughput":{"ReadCapacityUnits":5,"WriteCapacityUnits":5}}]' \
  --provisioned-throughput ReadCapacityUnits=5,WriteCapacityUnits=5 2>/dev/null && echo "  view_history ✓" || echo "  view_history (exists)"

echo ""
echo "Creating S3 buckets..."
$S3 s3 mb s3://rewind-raw 2>/dev/null && echo "  rewind-raw ✓" || echo "  rewind-raw (exists)"
$S3 s3 mb s3://rewind-videos 2>/dev/null && echo "  rewind-videos ✓" || echo "  rewind-videos (exists)"

echo "  Setting CORS on buckets..."
for bucket in rewind-raw rewind-videos; do
  $S3 s3api put-bucket-cors --bucket $bucket --cors-configuration '{
    "CORSRules": [{"AllowedOrigins":["*"],"AllowedMethods":["GET","PUT","POST","HEAD"],"AllowedHeaders":["*"],"ExposeHeaders":["ETag"]}]
  }' 2>/dev/null
done
echo "  CORS ✓"

echo ""
echo "Creating SQS queues..."
$SQS create-queue --queue-name transcode-jobs 2>/dev/null && echo "  transcode-jobs ✓" || echo "  transcode-jobs (exists)"

# Search-index sync queues (FIFO). In the cloud these are fed by an EventBridge Pipe off the videos
# DynamoDB stream; locally nothing feeds them (no Pipes in LocalStack), so they exist for parity and
# for manually exercising the search consumer (set STREAM_QUEUE_URL + SQS_ENDPOINT on the service).
$SQS create-queue --queue-name search-index-events-dlq.fifo \
  --attributes FifoQueue=true 2>/dev/null && echo "  search-index-events-dlq.fifo ✓" || echo "  search-index-events-dlq.fifo (exists)"
$SQS create-queue --queue-name search-index-events.fifo \
  --attributes FifoQueue=true,ContentBasedDeduplication=true 2>/dev/null && echo "  search-index-events.fifo ✓" || echo "  search-index-events.fifo (exists)"

# Delete-cleanup queues (FIFO). Like the search-index queues, in the cloud these are fed by an
# EventBridge Pipe off the videos stream (filtered to soft-deletes); locally nothing feeds them
# (no Pipes in LocalStack) — they exist for parity and for exercising the worker by hand.
$SQS create-queue --queue-name delete-cleanup-dlq.fifo \
  --attributes FifoQueue=true 2>/dev/null && echo "  delete-cleanup-dlq.fifo ✓" || echo "  delete-cleanup-dlq.fifo (exists)"
$SQS create-queue --queue-name delete-cleanup-events.fifo \
  --attributes FifoQueue=true,ContentBasedDeduplication=true 2>/dev/null && echo "  delete-cleanup-events.fifo ✓" || echo "  delete-cleanup-events.fifo (exists)"

echo ""
echo "Local dev environment ready!"
