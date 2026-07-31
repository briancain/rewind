// Link-preview (Open Graph + Twitter Card) metadata for the watch page.
//
// Why this exists: Slack/Discord/Twitter "unfurl" a pasted link by fetching the raw HTML and
// reading <meta> tags — they do NOT run JavaScript. The rest of this app is client-rendered, so a
// crawler only ever saw the static site-wide <head> ("Rewind" / "Video streaming platform") and
// every link unfurled identically. The watch page's Server Component calls `buildWatchMetadata`
// from `generateMetadata` so a crawler gets per-video title + description + a large thumbnail.
//
// This module is kept PURE (no fetch, no Next runtime, only a `Metadata` type import) so the gating
// and URL construction unit-test without a server or network. The I/O (fetching the video from
// catalog) lives at the edge in the page's `generateMetadata`.

import type { Metadata } from "next";

const SITE_NAME = "Rewind";
const SITE_DESCRIPTION = "Video streaming platform";

/** The subset of catalog's `GET /videos/{id}` response the preview needs. */
export interface VideoMetaInput {
  title: string;
  description: string;
  visibility: string; // "public" | "unlisted" | "private"
  status: string; // draft | processing | published | failed | deleted
  channel_id: string;
  // Bare S3 object key (e.g. "thumbnails/{id}/thumb.0000001.jpg"), NOT a presigned URL. Absent
  // until the transcode pipeline publishes the video. Served publicly via the content CDN.
  thumbnail_url?: string | null;
}

export interface MetadataOptions {
  /** Canonical site origin for og:url + metadataBase, e.g. "https://watch.example.com". */
  siteBase: string;
  /** Content-CDN origin that fronts the videos bucket, e.g. "https://cdn.example.com". */
  cdnBase: string;
}

/**
 * Whether a video should reveal a rich preview (title/description/thumbnail) to a crawler.
 * - public   → yes (in the feed + search + direct link).
 * - unlisted → yes: it's shareable by direct link (hidden only from feed/search), and unfurling a
 *   link someone deliberately shared matches the intent (and how YouTube treats unlisted).
 * - private  → NO: never leak an owner-only video's title/thumbnail to a channel.
 * - deleted / anything else → NO.
 * Kept pure so the gating is unit-tested directly.
 */
export function shouldExposePreview(
  visibility: string | undefined,
  status: string | undefined
): boolean {
  if (status === "deleted") return false;
  return visibility === "public" || visibility === "unlisted";
}

/**
 * Build the public CDN URL for a thumbnail from its stored S3 key. The `videos` bucket is the
 * CloudFront (`cdn.*`) origin and public/unlisted objects are served unsigned, so
 * `${cdnBase}/${key}` is a stable, crawler-fetchable, cacheable image URL — unlike the short-lived
 * presigned URL the player uses. Returns undefined when there's no key or no configured CDN base.
 */
export function thumbnailCdnUrl(
  thumbnailKey: string | null | undefined,
  cdnBase: string | undefined
): string | undefined {
  if (!thumbnailKey || !cdnBase) return undefined;
  const base = cdnBase.replace(/\/+$/, "");
  const key = thumbnailKey.replace(/^\/+/, "");
  if (!key) return undefined;
  return `${base}/${key}`;
}

/** The generic site card — used for private/deleted/missing videos so we never leak metadata. */
function genericMetadata(siteBase: string): Metadata {
  const base = siteBase ? { metadataBase: safeUrl(siteBase) } : {};
  return {
    ...base,
    title: SITE_NAME,
    description: SITE_DESCRIPTION,
    openGraph: {
      title: SITE_NAME,
      description: SITE_DESCRIPTION,
      siteName: SITE_NAME,
      type: "website",
      ...(siteBase ? { url: siteBase } : {}),
    },
    twitter: {
      card: "summary",
      title: SITE_NAME,
      description: SITE_DESCRIPTION,
    },
  };
}

function safeUrl(s: string): URL | undefined {
  try {
    return new URL(s);
  } catch {
    return undefined;
  }
}

/**
 * Build the page metadata for a watch page. `video` is null when the fetch failed / the video was
 * not found (or is a deleted tombstone, which catalog 404s). Returns a rich summary_large_image
 * card for public/unlisted videos, otherwise the generic site card.
 */
export function buildWatchMetadata(
  video: VideoMetaInput | null,
  videoId: string,
  opts: MetadataOptions
): Metadata {
  const { siteBase, cdnBase } = opts;

  if (!video || !shouldExposePreview(video.visibility, video.status)) {
    return genericMetadata(siteBase);
  }

  const title = video.title?.trim() || "Untitled video";
  const description = video.description?.trim() || SITE_DESCRIPTION;
  const url = siteBase ? `${siteBase.replace(/\/+$/, "")}/watch/${videoId}` : undefined;
  const image = thumbnailCdnUrl(video.thumbnail_url, cdnBase);

  const images = image
    ? [{ url: image, width: 1280, height: 720, alt: title }]
    : undefined;

  return {
    ...(siteBase && safeUrl(siteBase) ? { metadataBase: safeUrl(siteBase) } : {}),
    title,
    description,
    openGraph: {
      title,
      description,
      siteName: SITE_NAME,
      type: "video.other",
      ...(url ? { url } : {}),
      ...(images ? { images } : {}),
    },
    twitter: {
      // Large image card even without in-Slack playback — the big thumbnail + title is the goal.
      card: image ? "summary_large_image" : "summary",
      title,
      description,
      ...(image ? { images: [image] } : {}),
    },
  };
}
