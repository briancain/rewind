import {
  buildWatchMetadata,
  shouldExposePreview,
  thumbnailCdnUrl,
  type VideoMetaInput,
} from "@/lib/metadata";

const OPTS = { siteBase: "https://watch.example.com", cdnBase: "https://cdn.example.com" };

const publicVideo: VideoMetaInput = {
  title: "This is what 1999 felt like",
  description: "A retro synthwave journey",
  visibility: "public",
  status: "published",
  channel_id: "chan-1",
  thumbnail_url: "thumbnails/vid-1/thumb.0000001.jpg",
};

describe("shouldExposePreview", () => {
  it("exposes public and unlisted videos", () => {
    expect(shouldExposePreview("public", "published")).toBe(true);
    expect(shouldExposePreview("unlisted", "published")).toBe(true);
  });

  it("never exposes private videos", () => {
    expect(shouldExposePreview("private", "published")).toBe(false);
  });

  it("never exposes a deleted tombstone even if visibility is public", () => {
    expect(shouldExposePreview("public", "deleted")).toBe(false);
  });

  it("does not expose unknown/undefined visibility", () => {
    expect(shouldExposePreview(undefined, "published")).toBe(false);
    expect(shouldExposePreview("weird", "published")).toBe(false);
  });
});

describe("thumbnailCdnUrl", () => {
  it("joins the CDN base and bare key", () => {
    expect(thumbnailCdnUrl("thumbnails/v/t.jpg", "https://cdn.example.com")).toBe(
      "https://cdn.example.com/thumbnails/v/t.jpg"
    );
  });

  it("normalizes a trailing slash on the base and a leading slash on the key", () => {
    expect(thumbnailCdnUrl("/thumbnails/v/t.jpg", "https://cdn.example.com/")).toBe(
      "https://cdn.example.com/thumbnails/v/t.jpg"
    );
  });

  it("returns undefined when the key or the CDN base is missing", () => {
    expect(thumbnailCdnUrl(null, "https://cdn.example.com")).toBeUndefined();
    expect(thumbnailCdnUrl("thumbnails/v/t.jpg", "")).toBeUndefined();
    expect(thumbnailCdnUrl("", "https://cdn.example.com")).toBeUndefined();
  });
});

describe("buildWatchMetadata", () => {
  it("builds a rich summary_large_image card for a public video", () => {
    const m = buildWatchMetadata(publicVideo, "vid-1", OPTS);
    expect(m.title).toBe("This is what 1999 felt like");
    expect(m.description).toBe("A retro synthwave journey");
    expect(m.openGraph?.title).toBe("This is what 1999 felt like");
    expect(m.openGraph?.siteName).toBe("Rewind");
    // og:url points at the canonical watch page on the site origin.
    expect((m.openGraph as { url?: string })?.url).toBe("https://watch.example.com/watch/vid-1");
    // og:image is the PUBLIC CDN thumbnail URL (not a presigned URL).
    const images = m.openGraph?.images as Array<{ url: string }>;
    expect(images[0].url).toBe("https://cdn.example.com/thumbnails/vid-1/thumb.0000001.jpg");
    expect((m.twitter as { card?: string })?.card).toBe("summary_large_image");
    const twImages = (m.twitter as { images?: string[] })?.images;
    expect(twImages?.[0]).toBe("https://cdn.example.com/thumbnails/vid-1/thumb.0000001.jpg");
  });

  it("exposes an unlisted video (shareable by link)", () => {
    const m = buildWatchMetadata({ ...publicVideo, visibility: "unlisted" }, "vid-1", OPTS);
    expect(m.title).toBe("This is what 1999 felt like");
    expect((m.twitter as { card?: string })?.card).toBe("summary_large_image");
  });

  it("falls back to the generic site card for a private video (no leak)", () => {
    const m = buildWatchMetadata({ ...publicVideo, visibility: "private" }, "vid-1", OPTS);
    expect(m.title).toBe("Rewind");
    expect(m.description).toBe("Video streaming platform");
    // No private title/thumbnail leaks into the card.
    expect(m.openGraph?.images).toBeUndefined();
    expect((m.twitter as { card?: string })?.card).toBe("summary");
  });

  it("falls back to the generic card when the video is null (not found / fetch failed)", () => {
    const m = buildWatchMetadata(null, "vid-1", OPTS);
    expect(m.title).toBe("Rewind");
    expect(m.openGraph?.images).toBeUndefined();
  });

  it("emits a title-only card (summary) when a public video has no thumbnail yet", () => {
    const m = buildWatchMetadata({ ...publicVideo, thumbnail_url: null }, "vid-1", OPTS);
    expect(m.title).toBe("This is what 1999 felt like");
    expect(m.openGraph?.images).toBeUndefined();
    // No image → downgrade to the small-card type so we don't advertise a broken large image.
    expect((m.twitter as { card?: string })?.card).toBe("summary");
  });

  it("uses a placeholder title and the site description for empty fields", () => {
    const m = buildWatchMetadata(
      { ...publicVideo, title: "  ", description: "" },
      "vid-1",
      OPTS
    );
    expect(m.title).toBe("Untitled video");
    expect(m.description).toBe("Video streaming platform");
  });
});
