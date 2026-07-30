# Test Data

Place test videos in `videos/`. They are git-ignored and not committed to the repo.

## Current test videos

- `Apple - 1984.mp4`
- `nc101_hackers.mp4`
- `Rick_Astley_Never_Gonna_Give_You_Up.mp4`
- `experimentsinmotiongraphics.mp4`

## Catalog export (cloud snapshot)

Snapshot of the dev video catalog metadata, exported from the `rewind-dev-videos` DynamoDB table.
Use it to re-create the catalog after a clean-slate wipe (re-upload to exercise the transcode
pipeline) or as a fixture for a canary.

- `catalog-export.json` — cleaned metadata (title, description, genre, tags, visibility, status,
  `s3_key`, `manifest_url`, `thumbnail_url`, duration, created_at), one object per video.
- `catalog-export-raw-ddb.json` — raw DynamoDB `scan` output (re-importable via `batch-write-item`).

5 videos at export time; note `THE GOAT` has no local source file under `videos/` yet, and two
legacy items had no explicit `visibility` (the app treats absent as `public`).
