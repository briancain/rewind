jest.mock("@/lib/api", () => ({ svc: jest.fn() }));

import { svc } from "@/lib/api";
import { nextOffset, prevOffset, fetchChannel } from "@/lib/surf";

const mockSvc = svc as jest.Mock;

describe("offset navigation", () => {
  it("nextOffset increments without bound (server-side shuffle wraps)", () => {
    expect(nextOffset(0)).toBe(1);
    expect(nextOffset(41)).toBe(42);
  });

  it("prevOffset decrements but clamps at 0", () => {
    expect(prevOffset(3)).toBe(2);
    expect(prevOffset(1)).toBe(0);
    expect(prevOffset(0)).toBe(0);
  });
});

describe("fetchChannel", () => {
  beforeEach(() => mockSvc.mockReset());

  function happyPath() {
    mockSvc.mockImplementation((service: string, path: string) => {
      if (service === "catalog")
        return Promise.resolve({ video_id: "v1", channel_id: "c1", title: "Test Clip", created_at: "2026-01-01T00:00:00Z" });
      if (service === "identity") return Promise.resolve({ display_name: "Alice" });
      if (service === "social") return Promise.resolve({ likes: 1, dislikes: 2, views: 3, comment_count: 4 });
      if (service === "streaming" && path.includes("stream-url"))
        return Promise.resolve({ url: "https://cdn/hls/v1/master.m3u8" });
      if (service === "streaming" && path.includes("thumbnail-url"))
        return Promise.resolve({ url: "https://cdn/thumbnails/v1.jpg" });
      return Promise.reject(new Error(`unexpected ${service} ${path}`));
    });
  }

  it("composes the video, channel name, stats, and URLs", async () => {
    happyPath();
    const ch = await fetchChannel(777, 2);

    expect(mockSvc).toHaveBeenCalledWith("catalog", "/videos/surf?seed=777&offset=2");
    expect(ch.video.video_id).toBe("v1");
    expect(ch.video.title).toBe("Test Clip");
    expect(ch.channelName).toBe("Alice");
    expect(ch.stats).toEqual({ likes: 1, dislikes: 2, views: 3, comment_count: 4 });
    expect(ch.streamUrl).toBe("https://cdn/hls/v1/master.m3u8");
    expect(ch.posterUrl).toBe("https://cdn/thumbnails/v1.jpg");
  });

  it("degrades gracefully when secondary lookups fail", async () => {
    mockSvc.mockImplementation((service: string, path: string) => {
      if (service === "catalog")
        return Promise.resolve({ video_id: "v1", channel_id: "c1", title: "Test Clip", created_at: "2026-01-01T00:00:00Z" });
      if (service === "identity") return Promise.reject(new Error("user lookup down"));
      if (service === "social") return Promise.reject(new Error("stats down"));
      if (service === "streaming" && path.includes("stream-url")) return Promise.reject(new Error("no stream"));
      if (service === "streaming" && path.includes("thumbnail-url")) return Promise.reject(new Error("no thumb"));
      return Promise.reject(new Error("unexpected"));
    });

    const ch = await fetchChannel(1, 0);
    expect(ch.video.video_id).toBe("v1"); // primary still succeeds
    expect(ch.channelName).toBe("");
    expect(ch.stats).toBeNull();
    expect(ch.streamUrl).toBe("");
    expect(ch.posterUrl).toBe("");
  });

  it("rejects when the surf endpoint has no video (empty platform)", async () => {
    mockSvc.mockImplementation((service: string) => {
      if (service === "catalog") return Promise.reject(new Error("no videos available"));
      return Promise.resolve({});
    });
    await expect(fetchChannel(1, 0)).rejects.toThrow();
  });
});
