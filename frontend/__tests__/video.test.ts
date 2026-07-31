import { playbackState, shouldPollForReadiness } from "@/lib/video";

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
