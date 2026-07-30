#!/bin/bash
# scripts/invite.sh — Generate and manage invite codes for Rewind
set -e

ENDPOINT="${DYNAMODB_ENDPOINT:-http://localhost:8000}"
REGION="${AWS_REGION:-us-west-2}"
TABLE="${TABLE_PREFIX:-}invite_codes"
DDB="aws dynamodb --endpoint-url $ENDPOINT --region $REGION"

usage() {
  echo "Usage: ./scripts/invite.sh <command> [args]"
  echo ""
  echo "Commands:"
  echo "  generate [N]   Generate N invite codes (default: 1)"
  echo "  list           List all unused invite codes"
  echo ""
  echo "Environment:"
  echo "  DYNAMODB_ENDPOINT  DynamoDB endpoint (default: http://localhost:8000)"
  echo "  AWS_REGION         AWS region (default: us-west-2)"
  exit 1
}

generate() {
  local count="${1:-1}"
  echo "Generating $count invite code(s)..."
  for i in $(seq 1 "$count"); do
    code="REWIND-$(uuidgen | tr '[:upper:]' '[:lower:]' | cut -c1-8)"
    now=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
    $DDB put-item --table-name "$TABLE" --item "{
      \"code\": {\"S\": \"$code\"},
      \"created_at\": {\"S\": \"$now\"},
      \"used\": {\"BOOL\": false}
    }" > /dev/null
    echo "  $code"
  done
}

list_codes() {
  echo "Unused invite codes:"
  $DDB scan --table-name "$TABLE" \
    --filter-expression "used = :f" \
    --expression-attribute-values '{":f": {"BOOL": false}}' \
    --output json | python3 -c "
import sys, json
data = json.load(sys.stdin)
for item in data.get('Items', []):
    code = item['code']['S']
    created = item.get('created_at', {}).get('S', '?')
    print(f'  {code}  (created: {created})')
if not data.get('Items'):
    print('  (none)')
"
}

case "${1:-}" in
  generate) generate "$2" ;;
  list) list_codes ;;
  *) usage ;;
esac
