#!/bin/bash
# scripts/seed-local.sh — Seed the LOCAL dev stack (DynamoDB Local + LocalStack S3) with the
# testdata catalog so you can exercise the full browse / surf / watch flow without manually
# uploading videos through the UI.
#
# What it does (idempotent — safe to re-run):
#   1. Uploads the 4 testdata source videos to s3://rewind-videos/videos/{id}/video.mp4
#   2. Generates a ~25%-mark thumbnail per video via ffmpeg (optional) -> s3://rewind-videos/thumbnails/{id}/thumb.jpg
#   3. Writes the catalog rows into the `videos` table (status=published, visibility=public,
#      NO manifest_url -> local streaming presigns the MP4, which is what plays locally)
#   4. Seeds the owning channel user so the channel name renders in the UI
#
# Note: "THE GOAT" (27e2...) from the export is intentionally skipped — it's a cloud-only HLS row
# (CloudFront manifest) with no local source file.
#
# Prereqs:
#   - Local data containers up + tables/buckets created. Easiest: run ./scripts/dev.sh once (it
#     starts containers + runs local-setup.sh), or bring up just the data plane:
#       finch compose up -d dynamodb-local localstack && ./scripts/local-setup.sh
#   - testdata/videos/*.mp4 present (git-ignored — see testdata/README.md)
#   - ffmpeg (optional; without it videos still play, just with the placeholder ▶ thumbnail)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RAW="$ROOT/testdata/catalog-export-raw-ddb.json"
VIDEODIR="$ROOT/testdata/videos"

export AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test AWS_DEFAULT_REGION=us-west-2
DDB="aws dynamodb --endpoint-url http://localhost:8000 --region us-west-2"
S3="aws --endpoint-url http://localhost:4566 --region us-west-2"
BUCKET=rewind-videos

# All testdata videos share this owner (channel_id from the export). Seed a matching user so the
# UI can resolve the channel display name.
CHANNEL_ID="6281cb7c-006f-416a-9d5c-9286b3ef28a7"
CHANNEL_NAME="Rewind Archive"

# video_id -> local source filename. THE GOAT (27e2...) is omitted on purpose (no local file).
TARGET_IDS="b210a9a9-9f3d-4084-bbbf-e7b2d71c4354 aa938f69-b181-4406-9c34-13dd1e9b453b 497d18fe-91e5-460f-85b4-33525c0578f7 ae3ff512-d5ee-40bf-81a7-8381b2d0388e"

src_file_for() {
  case "$1" in
    b210a9a9-*) echo "Rick_Astley_Never_Gonna_Give_You_Up.mp4" ;;
    aa938f69-*) echo "Apple - 1984.mp4" ;;
    497d18fe-*) echo "nc101_hackers.mp4" ;;
    ae3ff512-*) echo "experimentsinmotiongraphics.mp4" ;;
    *) echo "" ;;
  esac
}

# --- Preflight ---
if [ ! -f "$RAW" ]; then echo "✗ missing $RAW"; exit 1; fi
if ! curl -s -o /dev/null http://localhost:8000 2>/dev/null; then
  echo "✗ DynamoDB Local not reachable on :8000. Start the stack first (./scripts/dev.sh)."; exit 1
fi
if ! curl -s -o /dev/null http://localhost:4566 2>/dev/null; then
  echo "✗ LocalStack not reachable on :4566. Start the stack first (./scripts/dev.sh)."; exit 1
fi
HAVE_FFMPEG=0; command -v ffmpeg >/dev/null 2>&1 && HAVE_FFMPEG=1
[ "$HAVE_FFMPEG" -eq 0 ] && echo "⚠ ffmpeg not found — skipping thumbnails (videos will use the ▶ placeholder)."

echo "▶ Seeding local catalog from testdata..."

for vid in $TARGET_IDS; do
  file="$(src_file_for "$vid")"
  src="$VIDEODIR/$file"
  if [ ! -f "$src" ]; then
    echo "  ⚠ $file not found in testdata/videos — skipping $vid"
    continue
  fi

  # Pull s3_key / thumbnail_url / duration for this row from the raw export.
  read -r s3_key thumb_key duration < <(python3 - "$RAW" "$vid" <<'PY'
import json, sys
items = json.load(open(sys.argv[1]))["Items"]
it = next(i for i in items if i["video_id"]["S"] == sys.argv[2])
print(it["s3_key"]["S"], it["thumbnail_url"]["S"], it["duration_seconds"]["N"])
PY
)

  echo "  • ${file}"
  $S3 s3 cp "$src" "s3://$BUCKET/$s3_key" >/dev/null && echo "      ✓ video -> $s3_key"

  if [ "$HAVE_FFMPEG" -eq 1 ]; then
    # Grab a frame at ~25% of the duration.
    ts=$(python3 -c "print(max(1, int(float('$duration')) // 4))")
    if ffmpeg -y -loglevel error -ss "$ts" -i "$src" -frames:v 1 -q:v 3 /tmp/seed_thumb.jpg </dev/null 2>/dev/null; then
      $S3 s3 cp /tmp/seed_thumb.jpg "s3://$BUCKET/$thumb_key" >/dev/null && echo "      ✓ thumbnail -> $thumb_key"
    else
      echo "      ⚠ thumbnail generation failed (continuing)"
    fi
  fi

  # Write the catalog row (raw DDB item, minus any manifest_url so local streaming presigns the
  # MP4). Normalize visibility to public — some legacy export rows predate the visibility field,
  # and a strict `visibility = public` feed filter would otherwise hide them from the home grid.
  python3 - "$RAW" "$vid" > /tmp/seed_item.json <<'PY'
import json, sys
items = json.load(open(sys.argv[1]))["Items"]
it = next(i for i in items if i["video_id"]["S"] == sys.argv[2])
it.pop("manifest_url", None)
it["visibility"] = {"S": "public"}
json.dump(it, sys.stdout)
PY
  $DDB put-item --table-name videos --item file:///tmp/seed_item.json >/dev/null && echo "      ✓ catalog row"
done

# Seed the owning channel user (so the channel name renders). Harmless if it already exists.
$DDB put-item --table-name users --item "{
  \"user_id\": {\"S\": \"$CHANNEL_ID\"},
  \"email\": {\"S\": \"archive@rewind.local\"},
  \"display_name\": {\"S\": \"$CHANNEL_NAME\"},
  \"created_at\": {\"S\": \"2026-06-03T00:00:00+00:00\"}
}" >/dev/null && echo "  ✓ channel user: $CHANNEL_NAME"

rm -f /tmp/seed_thumb.jpg /tmp/seed_item.json

COUNT=$($DDB scan --table-name videos --select COUNT --query Count --output text 2>/dev/null || echo "?")
echo ""
echo "✓ Done. videos table now has $COUNT row(s)."
echo ""
echo "Next:"
echo "  1. Make sure services + frontend are up:  ./scripts/dev.sh"
echo "  2. Generate an invite to register a test account:  ./scripts/invite.sh"
echo "  3. Open http://localhost:3000 → register → click 🏄 Surf and flip channels."
