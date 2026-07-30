#!/bin/bash
# scripts/admin-reset-password.sh — Break-glass password reset (no email).
#
# Rewind has no self-serve password recovery (personal demo). If you get
# locked out, an admin with DynamoDB access resets the password directly with this script:
#   1. Prompts for the new password (hidden input; never in shell history or the process list).
#   2. Generates an argon2 hash using the identity service's OWN hasher (guaranteed compatible).
#   3. Looks up the user_id by email.
#   4. Overwrites password_hash on the user record.
#   5. Invalidates all existing sessions for that user (forces a fresh login).
#
# Usage:
#   ./scripts/admin-reset-password.sh <email>
#
# The new password is entered at an interactive hidden prompt — it is never passed as an argument
# (so it can't leak via `history` or `ps`) and is fed to the hasher over stdin.
set -euo pipefail

EMAIL="${1:-}"
if [ -z "$EMAIL" ]; then
  echo "usage: $0 <email>" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="rewind"
REGION="us-west-2"
TF_DIR="$ROOT/infra/environments/dev/us-west-2"

TABLE_PREFIX=$(cd "$TF_DIR" && terraform output -raw dynamodb_table_prefix)
USERS_TABLE="${TABLE_PREFIX}users"
SESSIONS_TABLE="${TABLE_PREFIX}sessions"
DOMAIN=$(cd "$TF_DIR" && terraform output -raw domain)

# Hidden, confirmed password prompt (no echo, no history, no argv).
read -r -s -p "New password for $EMAIL: " NEWPW; echo
read -r -s -p "Confirm new password:      " CONFIRM; echo
if [ "$NEWPW" != "$CONFIRM" ]; then
  echo "✗ passwords do not match" >&2
  exit 1
fi
if [ "${#NEWPW}" -lt 8 ]; then
  echo "✗ new password must be at least 8 characters" >&2
  exit 1
fi

echo "▶ Generating argon2 hash with the identity service's hasher..."
# printf is a shell builtin, so the password is not visible in `ps`; it reaches the hasher via stdin.
HASH=$(printf '%s' "$NEWPW" | (cd "$ROOT/services" && cargo run -q -p identity -- hash-password))
unset NEWPW CONFIRM
if [ -z "$HASH" ]; then
  echo "✗ failed to generate password hash" >&2
  exit 1
fi

echo "▶ Looking up user by email ($EMAIL)..."
USER_ID=$(aws dynamodb query \
  --table-name "$USERS_TABLE" --index-name email-index \
  --key-condition-expression "email = :e" \
  --expression-attribute-values "{\":e\":{\"S\":\"$EMAIL\"}}" \
  --profile "$PROFILE" --region "$REGION" \
  --query "Items[0].user_id.S" --output text)
if [ "$USER_ID" = "None" ] || [ -z "$USER_ID" ]; then
  echo "✗ no user found with email $EMAIL" >&2
  exit 1
fi
echo "  user_id: $USER_ID"

echo "▶ Updating password_hash..."
aws dynamodb update-item \
  --table-name "$USERS_TABLE" \
  --key "{\"user_id\":{\"S\":\"$USER_ID\"}}" \
  --update-expression "SET password_hash = :h" \
  --expression-attribute-values "{\":h\":{\"S\":\"$HASH\"}}" \
  --profile "$PROFILE" --region "$REGION"
echo "  ✓ password updated"

# Invalidate existing sessions. Uses Scan (not the user-id-index GSI) so this works regardless of
# whether the GSI has been deployed yet.
echo "▶ Invalidating existing sessions..."
TOKENS=$(aws dynamodb scan \
  --table-name "$SESSIONS_TABLE" \
  --filter-expression "user_id = :u" \
  --expression-attribute-values "{\":u\":{\"S\":\"$USER_ID\"}}" \
  --profile "$PROFILE" --region "$REGION" \
  --query "Items[].session_token.S" --output text)
COUNT=0
for t in $TOKENS; do
  aws dynamodb delete-item \
    --table-name "$SESSIONS_TABLE" \
    --key "{\"session_token\":{\"S\":\"$t\"}}" \
    --profile "$PROFILE" --region "$REGION"
  COUNT=$((COUNT + 1))
done
echo "  ✓ invalidated $COUNT session(s)"

echo ""
echo "✓ Done. Log in at https://${DOMAIN} with your new password."
