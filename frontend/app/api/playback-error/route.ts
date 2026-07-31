import type { NextRequest } from "next/server";

// Client-side playback-failure beacon. VideoPlayer POSTs here when the media element fires an
// `error` (hls.js fatal error / MediaError) — cases where streaming + CloudFront returned 200s but
// the video still won't play in the browser (codec, CORS, manifest parse), which are otherwise
// invisible server-side.
//
// We log ONE flat structured line to stderr (matching instrumentation.ts's shape, so it lands under
// $.log_processed for the CloudWatch filter) with a stable `event: "playback_error"` marker that the
// `frontend-playback-errors` metric filter counts. Only a fixed set of bounded fields is logged
// (message is truncated by the client builder) so a client can't inflate log volume arbitrarily.
// The endpoint is intentionally unauthenticated (anonymous users can watch public videos); the
// alarm threshold absorbs the low baseline.
export async function POST(req: NextRequest) {
  let body: Record<string, unknown> = {};
  try {
    body = await req.json();
  } catch {
    // ignore malformed body — still emit a record below
  }

  const message =
    typeof body.message === "string" && body.message.length > 0
      ? body.message.slice(0, 200)
      : "playback error";

  console.error(
    JSON.stringify({
      level: "error",
      logger: "frontend",
      event: "playback_error",
      video_id: typeof body.video_id === "string" ? body.video_id : "unknown",
      error_code: typeof body.error_code === "number" ? body.error_code : null,
      is_hls: typeof body.is_hls === "boolean" ? body.is_hls : null,
      message,
    })
  );

  return new Response(null, { status: 204 });
}
