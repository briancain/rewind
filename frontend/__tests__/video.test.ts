import { playbackState, shouldPollForReadiness, buildPlaybackErrorBeacon } from "@/lib/video";

describe("playbackState", () => {
  it("reports failed regardless of stream url", () => {
    expect(playbackState("failed", false)).toBe("failed");
    expect(playbackState("failed", true)).toBe("failed");
  });

  it("reports processing for draft/processing status", () => {
    expect(playbackState("draft", false)).toBe("processing");
    expect(playbackState("processing", false)).toBe("processing");
  });

  it("reports processing when no stream url yet, even if published", () => {
    // published but streaming hasn't returned a URL (edge/replication lag) -> still show processing
    expect(playbackState("published", false)).toBe("processing");
  });

  it("does NOT treat the bogus legacy 'pending' status as ready", () => {
    // regression: the old check was `status === "pending"`, which never matched a real status
    expect(playbackState("pending", false)).toBe("processing");
  });

  it("reports ready only when published-ish AND a stream url exists", () => {
    expect(playbackState("published", true)).toBe("ready");
  });
});

describe("shouldPollForReadiness", () => {
  it("polls while processing with no url", () => {
    expect(shouldPollForReadiness("processing", false)).toBe(true);
    expect(shouldPollForReadiness("draft", false)).toBe(true);
    expect(shouldPollForReadiness("published", false)).toBe(true);
  });

  it("stops once a url exists", () => {
    expect(shouldPollForReadiness("processing", true)).toBe(false);
  });

  it("does not poll for terminal states", () => {
    expect(shouldPollForReadiness("failed", false)).toBe(false);
    expect(shouldPollForReadiness("deleted", false)).toBe(false);
  });
});

describe("buildPlaybackErrorBeacon", () => {
  it("builds a beacon from a MediaError on an HLS source", () => {
    const b = buildPlaybackErrorBeacon("vid-1", "https://cdn/hls/vid-1/x.m3u8", {
      code: 4,
      message: "MEDIA_ERR_SRC_NOT_SUPPORTED",
    });
    expect(b).toEqual({
      video_id: "vid-1",
      is_hls: true,
      error_code: 4,
      message: "MEDIA_ERR_SRC_NOT_SUPPORTED",
    });
  });

  it("falls back for a missing video id / null error and detects a non-HLS source", () => {
    const b = buildPlaybackErrorBeacon(undefined, "https://s3/mp4/x.mp4", null);
    expect(b.video_id).toBe("unknown");
    expect(b.is_hls).toBe(false);
    expect(b.error_code).toBeNull();
    expect(b.message).toBe("playback error");
  });

  it("truncates a long error message to 200 chars", () => {
    const b = buildPlaybackErrorBeacon("v", "a.m3u8", { code: 3, message: "x".repeat(500) });
    expect(b.message).toHaveLength(200);
  });
});
