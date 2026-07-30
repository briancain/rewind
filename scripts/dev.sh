#!/bin/bash
# scripts/dev.sh — One-command local dev environment
set -e

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PIDS=()

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
CYAN='\033[0;36m'
WHITE='\033[0;37m'
NC='\033[0m'

cleanup() {
  echo ""
  echo -e "${WHITE}Shutting down services...${NC}"
  for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null
  done
  lsof -ti:3000 2>/dev/null | xargs kill 2>/dev/null
  echo -e "${GREEN}✓ All services stopped. Containers still running.${NC}"
  echo "  Run './scripts/local-stop.sh' for full teardown."
  exit 0
}
trap cleanup SIGINT SIGTERM

wait_for_port() {
  local port=$1 name=$2 max=30
  for i in $(seq 1 $max); do
    if curl -s -o /dev/null http://localhost:$port 2>/dev/null; then
      return 0
    fi
    sleep 1
  done
  echo -e "${RED}✗ $name on port $port failed to start${NC}"
  return 1
}

# --- Step 1: Start containers ---
echo -e "${WHITE}▶ Starting infrastructure containers...${NC}"
cd "$ROOT"
finch compose up -d dynamodb-local localstack opensearch > /dev/null 2>&1
echo -e "${GREEN}  ✓ Containers up${NC}"

# --- Step 2: Wait for readiness ---
echo -e "${WHITE}▶ Waiting for infrastructure...${NC}"
wait_for_port 8000 "DynamoDB" && echo -e "${GREEN}  ✓ DynamoDB ready${NC}"
wait_for_port 4566 "LocalStack" && echo -e "${GREEN}  ✓ LocalStack ready${NC}"
wait_for_port 9200 "OpenSearch" && echo -e "${GREEN}  ✓ OpenSearch ready${NC}"

# --- Step 3: Create tables/buckets/queues ---
echo -e "${WHITE}▶ Running local-setup...${NC}"
"$ROOT/scripts/local-setup.sh" > /dev/null 2>&1
echo -e "${GREEN}  ✓ Tables, buckets, queues ready${NC}"

# --- Step 4: Build ---
echo -e "${WHITE}▶ Building services...${NC}"
cd "$ROOT/services"
cargo build --all 2>&1 | grep -E "Compiling|Finished|error" || true
echo -e "${GREEN}  ✓ Build complete${NC}"

# --- Step 5: Start services ---
echo -e "${WHITE}▶ Starting backend services...${NC}"
BIN="$ROOT/services/target/debug"
AWS="AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-west-2"
DDB="DYNAMODB_ENDPOINT=http://localhost:8000"
S3="S3_ENDPOINT=http://localhost:4566"
SQS="SQS_ENDPOINT=http://localhost:4566"

PREFIX_IDENTITY=$(printf '\033[0;31m[identity]\033[0m ')
PREFIX_CATALOG=$(printf '\033[0;32m[catalog]\033[0m ')
PREFIX_UPLOAD=$(printf '\033[0;33m[upload]\033[0m ')
PREFIX_STREAM=$(printf '\033[0;34m[stream]\033[0m ')
PREFIX_SOCIAL=$(printf '\033[0;35m[social]\033[0m ')
PREFIX_SEARCH=$(printf '\033[0;36m[search]\033[0m ')
PREFIX_TRANSCODE=$(printf '\033[0;37m[transcode]\033[0m ')

env PORT=8080 $DDB DISABLE_SES=1 $AWS "$BIN/identity" 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_IDENTITY" "$line"; done &
PIDS+=($!)
env PORT=8081 $DDB $AWS "$BIN/video-catalog" 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_CATALOG" "$line"; done &
PIDS+=($!)
env PORT=8082 $DDB $S3 $SQS $AWS "$BIN/upload" 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_UPLOAD" "$line"; done &
PIDS+=($!)
env PORT=8083 $DDB $S3 $AWS "$BIN/streaming" 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_STREAM" "$line"; done &
PIDS+=($!)
env PORT=8084 $DDB $AWS "$BIN/social" 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_SOCIAL" "$line"; done &
PIDS+=($!)
env PORT=8085 OPENSEARCH_ENDPOINT=http://localhost:9200 "$BIN/search" 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_SEARCH" "$line"; done &
PIDS+=($!)
env PORT=8086 $DDB $S3 $SQS DISABLE_MEDIACONVERT=1 SEARCH_ENDPOINT=http://localhost:8085 $AWS "$BIN/transcode" 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_TRANSCODE" "$line"; done &
PIDS+=($!)

# Wait for all services
for port in 8080 8081 8082 8083 8084 8085 8086; do
  wait_for_port $port "port $port"
done
echo -e "${GREEN}  ✓ All backend services healthy${NC}"

# --- Step 6: Start frontend ---
echo -e "${WHITE}▶ Starting frontend...${NC}"
cd "$ROOT/frontend"
PREFIX_FRONTEND=$(printf '\033[0;37m[frontend]\033[0m ')
npm run dev 2>&1 | while IFS= read -r line; do printf '%s%s\n' "$PREFIX_FRONTEND" "$line"; done &
PIDS+=($!)
wait_for_port 3000 "frontend"
echo -e "${GREEN}  ✓ Frontend ready${NC}"

# --- Done ---
echo ""
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo -e "${GREEN}  Rewind is running! http://localhost:3000${NC}"
echo -e "${GREEN}═══════════════════════════════════════════${NC}"
echo ""
echo "  identity:      http://localhost:8080"
echo "  video-catalog: http://localhost:8081"
echo "  upload:        http://localhost:8082"
echo "  streaming:     http://localhost:8083"
echo "  social:        http://localhost:8084"
echo "  search:        http://localhost:8085"
echo "  transcode:     http://localhost:8086"
echo ""
echo -e "  Press ${YELLOW}Ctrl+C${NC} to stop all services."
echo ""

wait
