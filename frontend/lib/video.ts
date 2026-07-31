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

// Client-side playback failure beacon. Even when streaming/CloudFront return 200s, playback can
// still fail in the browser (hls.js fatal error, unsupported codec, CORS on segments, manifest
// parse) — invisible server-side. VideoPlayer POSTs this payload to /api/playback-error on the
// media element's `error` event; the route handler logs it so a CloudWatch filter/alarm can catch
// a spike. Kept pure + bounded (message truncated) so it unit-tests and can't be abused to emit
// unbounded log volume.
export interface PlaybackErrorBeacon {
  video_id: string;
  is_hls: boolean;
  error_code: number | null;
  message: string;
}

export function buildPlaybackErrorBeacon(
  videoId: string | undefined,
  streamUrl: string,
  mediaError: { code?: number; message?: string } | null | undefined
): PlaybackErrorBeacon {
  const raw = mediaError?.message?.trim();
  return {
    video_id: videoId && videoId.trim() !== "" ? videoId : "unknown",
    is_hls: streamUrl.endsWith(".m3u8"),
    error_code: typeof mediaError?.code === "number" ? mediaError.code : null,
    message: (raw && raw.length > 0 ? raw : "playback error").slice(0, 200),
  };
}
