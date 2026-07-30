import { svc } from "@/lib/api";

// The surf "TV channel" data model + the pure navigation helpers and the fetch composition that
// the surf page builds on. Kept separate from the React component so the deterministic
// next/back arithmetic and the multi-service fetch fan-out are unit-testable.

export interface SurfVideo {
  video_id: string;
  title: string;
  channel_id: string;
  created_at: string;
  duration_seconds?: number;
  description?: string;
  genre?: string;
  tags?: string[];
}

export interface SurfStats {
  likes: number;
  dislikes: number;
  views: number;
  comment_count: number;
}

/** Everything needed to render one surf channel: the video, its channel name, social stats, and
 *  the (already-resolved) stream + poster URLs. */
export interface SurfChannel {
  video: SurfVideo;
  channelName: string;
  stats: SurfStats | null;
  streamUrl: string;
  posterUrl: string;
}

/** Advance to the next channel. The catalog surf endpoint applies a deterministic seeded shuffle
 *  and wraps with `offset % len`, so offsets are unbounded above. */
export function nextOffset(offset: number): number {
  return offset + 1;
}

/** Go back a channel, clamped at 0 (the first channel of this surf session). */
export function prevOffset(offset: number): number {
  return Math.max(0, offset - 1);
}

/**
 * Fetch a full surf channel for `(seed, offset)`: the shuffled video from the catalog, then — in
 * parallel — its channel display name, social stats, stream URL, and poster URL. The secondary
 * lookups degrade gracefully (a failed stat/name/poster yields a sensible default rather than
 * failing the whole channel). A missing video (404 from surf) rejects, so the caller can show the
 * empty state.
 */
export async function fetchChannel(seed: number, offset: number): Promise<SurfChannel> {
  const video = await svc<SurfVideo>(
    "catalog",
    `/videos/surf?seed=${seed}&offset=${offset}`,
  );

  const [channelName, stats, streamUrl, posterUrl] = await Promise.all([
    svc<{ display_name: string }>("identity", `/users/${video.channel_id}`)
      .then((u) => u.display_name)
      .catch(() => ""),
    svc<SurfStats>("social", `/videos/${video.video_id}/stats`).catch(() => null),
    svc<{ url: string }>("streaming", `/videos/${video.video_id}/stream-url`)
      .then((r) => r.url)
      .catch(() => ""),
    svc<{ url: string }>("streaming", `/videos/${video.video_id}/thumbnail-url`)
      .then((r) => r.url)
      .catch(() => ""),
  ]);

  return { video, channelName, stats, streamUrl, posterUrl };
}
