// Watch page — Server Component.
//
// This is the app's one server-rendered data path, and it exists specifically so link-preview
// crawlers (Slack/Discord/Twitter) get per-video Open Graph + Twitter Card tags in <head>. Next 16
// resolves `generateMetadata` synchronously into <head> for detected bots (Slackbot, Twitterbot,
// etc.), so a dynamically-rendered page still unfurls correctly. All the interactive UI stays in
// the client component `WatchClient`; this wrapper only fetches the video metadata for the tags.
//
// The player itself is unchanged — it still fetches streams/thumbnails client-side via short-lived
// presigned URLs. The only thing added here is a public CDN og:image for public/unlisted videos.

import type { Metadata } from "next";
import WatchClient from "./WatchClient";
import { buildWatchMetadata, type VideoMetaInput } from "@/lib/metadata";

// Catalog is reachable server-side via the build-time-inlined public URL (same one the browser
// uses); og:url + og:image bases come from the frontend's own site + CDN origins.
const CATALOG_BASE = process.env.NEXT_PUBLIC_CATALOG_URL || "http://localhost:8081";
const SITE_BASE = process.env.NEXT_PUBLIC_SITE_URL || "";
const CDN_BASE = process.env.NEXT_PUBLIC_CDN_URL || "";

// Fetch the video anonymously (a crawler has no session) with a short timeout so a slow/hung
// catalog can't stall the unfurl response. `no-store` keeps the tags current after a title/thumb
// edit. Any failure (404 for private-owner? no — private returns a row; 404 = missing/deleted,
// or a network error) resolves to null → the builder falls back to the generic site card.
async function fetchVideoMeta(id: string): Promise<VideoMetaInput | null> {
  try {
    const res = await fetch(`${CATALOG_BASE}/videos/${encodeURIComponent(id)}`, {
      cache: "no-store",
      signal: AbortSignal.timeout(3000),
    });
    if (!res.ok) return null;
    return (await res.json()) as VideoMetaInput;
  } catch {
    return null;
  }
}

export async function generateMetadata({
  params,
}: {
  params: Promise<{ id: string }>;
}): Promise<Metadata> {
  const { id } = await params;
  const video = await fetchVideoMeta(id);
  return buildWatchMetadata(video, id, { siteBase: SITE_BASE, cdnBase: CDN_BASE });
}

export default async function WatchPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  return <WatchClient id={id} />;
}
