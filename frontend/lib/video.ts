// Video playback readiness, derived from the catalog `status` and whether streaming has issued a
// URL yet. Kept pure (no React) so it unit-tests without rendering the watch page.
//
// A freshly-uploaded video is `draft` -> `processing` until the transcode pipeline publishes it;
// during that window streaming returns 409 (no manifest/asset yet), so `hasStreamUrl` is false.
// Previously the watch page checked `status === "pending"`, which is not a real status (the
// lifecycle is draft/processing/published/failed/deleted) — so that branch was dead and readiness
// leaned entirely on `hasStreamUrl`.

export type PlaybackState = "failed" | "processing" | "ready";

export function playbackState(
  status: string | undefined,
  hasStreamUrl: boolean
): PlaybackState {
  if (status === "failed") return "failed";
  if (!hasStreamUrl || status === "draft" || status === "processing") {
    return "processing";
  }
  return "ready";
}

// Whether the watch page should keep polling streaming for a URL: the video isn't failed/deleted
// and we don't have a URL yet.
export function shouldPollForReadiness(
  status: string | undefined,
  hasStreamUrl: boolean
): boolean {
  if (hasStreamUrl) return false;
  if (status === "failed" || status === "deleted") return false;
  return true;
}
