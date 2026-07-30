"use client";
import { useEffect, useRef, useState, useCallback } from "react";
import dynamic from "next/dynamic";
import Link from "next/link";
import { svc } from "@/lib/api";
import { useRequireAuth } from "@/lib/auth";
import { timeAgo, formatDuration } from "@/lib/format";
import { fetchChannel, nextOffset, prevOffset, type SurfChannel, type SurfStats } from "@/lib/surf";

// Client-only: the player loads Media Chrome + hls-video-element web components, which register
// custom elements on import and must not execute during SSR.
const VideoPlayer = dynamic(() => import("@/components/VideoPlayer"), { ssr: false });

export default function SurfPage() {
  // A single random seed per session fixes the shuffle, so Back/Next are stable and reversible.
  const [seed] = useState(() => Math.floor(Math.random() * 100000));
  const [offset, setOffset] = useState(0);
  const [channel, setChannel] = useState<SurfChannel | null>(null);
  const [tvOn, setTvOn] = useState(false);
  const [empty, setEmpty] = useState(false);
  const [loading, setLoading] = useState(true);

  // Channels are cached by offset so Back/Next (and auto-advance) are instant. The deterministic
  // shuffle means offset+1 is predictable, so we prefetch it in the background.
  const cacheRef = useRef<Record<number, SurfChannel>>({});
  const inflightRef = useRef<Set<number>>(new Set());
  const requireAuth = useRequireAuth();

  const prefetch = useCallback(
    (o: number) => {
      if (o < 0 || cacheRef.current[o] || inflightRef.current.has(o)) return;
      inflightRef.current.add(o);
      fetchChannel(seed, o)
        .then((ch) => {
          cacheRef.current[o] = ch;
        })
        .catch(() => {})
        .finally(() => inflightRef.current.delete(o));
    },
    [seed],
  );

  // Load the current channel (cache-first for snappy flips), then prefetch the next one.
  useEffect(() => {
    let cancelled = false;

    const cached = cacheRef.current[offset];
    if (cached) {
      setChannel(cached);
      setEmpty(false);
      setLoading(false);
      prefetch(nextOffset(offset));
      return;
    }

    setLoading(true);
    fetchChannel(seed, offset)
      .then((ch) => {
        if (cancelled) return;
        cacheRef.current[offset] = ch;
        setChannel(ch);
        setEmpty(false);
        setLoading(false);
        prefetch(nextOffset(offset));
      })
      .catch(() => {
        if (cancelled) return;
        setEmpty(true);
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [seed, offset, prefetch]);

  const goNext = useCallback(() => setOffset((o) => nextOffset(o)), []);
  const goBack = useCallback(() => setOffset((o) => prevOffset(o)), []);

  // Record a view (deduped per session) + watch history once playback passes 5s. Mirrors the
  // watch page, so surfed-and-watched videos count exactly like a normal watch.
  const handleWatched = useCallback(() => {
    const id = channel?.video.video_id;
    if (!id) return;
    const viewed = JSON.parse(sessionStorage.getItem("viewed") || "[]");
    if (!viewed.includes(id)) {
      svc("social", `/videos/${id}/view`, { method: "POST" }).catch(() => {});
      sessionStorage.setItem("viewed", JSON.stringify([...viewed, id]));
    }
    svc("social", `/videos/${id}/history`, { method: "POST" }).catch(() => {});
  }, [channel]);

  async function handleReaction(type: "like" | "dislike") {
    const id = channel?.video.video_id;
    if (!id) return;
    if (!requireAuth(`${type} this video`)) return;
    await svc("social", `/videos/${id}/${type}`, { method: "POST" });
    const stats = await svc<SurfStats>("social", `/videos/${id}/stats`).catch(() => null);
    if (stats) {
      setChannel((c) => (c ? { ...c, stats } : c));
      const cached = cacheRef.current[offset];
      if (cached) cacheRef.current[offset] = { ...cached, stats };
    }
  }

  if (empty) {
    return (
      <div className="max-w-2xl mx-auto text-center mt-20">
        <p className="text-neutral-400 text-lg">No videos to surf yet.</p>
        <Link href="/upload" className="inline-block mt-4 px-4 py-2 bg-red-600 rounded hover:bg-red-700">
          Upload the first video
        </Link>
      </div>
    );
  }

  const video = channel?.video;
  const stats = channel?.stats;
  const canPlay = Boolean(channel?.streamUrl);

  return (
    <div className="max-w-4xl mx-auto">
      {/* Streak / channel indicator */}
      <div className="flex items-center justify-between mb-3">
        <h1 className="text-lg font-bold flex items-center gap-2">
          📺 Surf
          <span className="text-xs font-normal text-neutral-500">Channel #{offset + 1}</span>
        </h1>
        <span className="text-xs text-neutral-500">
          {offset === 0 ? "Start of your surf session" : `🔥 ${offset} channel${offset === 1 ? "" : "s"} flipped`}
        </span>
      </div>

      {/* The "TV": screen + controls */}
      <div className="bg-black rounded-xl p-2 sm:p-3 ring-1 ring-neutral-800">
        <div className="relative w-full aspect-video bg-black rounded-lg overflow-hidden">
          {tvOn && canPlay ? (
            <VideoPlayer
              key={video!.video_id}
              streamUrl={channel!.streamUrl}
              posterUrl={channel!.posterUrl || undefined}
              autoPlay
              onWatched={handleWatched}
              onEnded={goNext}
            />
          ) : (
            // TV "off" screen — poster + power button. The click is the user gesture that unlocks
            // audio autoplay for the rest of the session.
            <div className="absolute inset-0 flex flex-col items-center justify-center gap-4">
              {channel?.posterUrl && (
                // eslint-disable-next-line @next/next/no-img-element
                <img src={channel.posterUrl} alt="" className="absolute inset-0 w-full h-full object-cover opacity-30" />
              )}
              {loading && !channel ? (
                <span className="relative text-neutral-500 text-sm animate-pulse">Tuning in…</span>
              ) : !canPlay && channel ? (
                <div className="relative text-center">
                  <p className="text-neutral-300 text-sm">📡 No signal on this channel</p>
                  <button onClick={goNext} className="mt-3 px-4 py-1.5 bg-neutral-700 rounded text-sm hover:bg-neutral-600">
                    Skip →
                  </button>
                </div>
              ) : (
                <button
                  onClick={() => setTvOn(true)}
                  aria-label="Turn on"
                  className="relative flex flex-col items-center gap-2 text-neutral-200 hover:text-white transition group"
                >
                  <span className="w-16 h-16 rounded-full bg-red-600 group-hover:bg-red-500 flex items-center justify-center text-3xl shadow-lg">
                    ⏻
                  </span>
                  <span className="text-sm font-medium">Turn on</span>
                </button>
              )}
            </div>
          )}
        </div>

        {/* Channel controls */}
        <div className="flex items-center justify-center gap-3 mt-2 sm:mt-3">
          <button
            onClick={goBack}
            disabled={offset === 0}
            className="px-5 py-2 rounded-full bg-neutral-800 text-sm hover:bg-neutral-700 disabled:opacity-40 disabled:cursor-not-allowed"
          >
            ← Back
          </button>
          <button
            onClick={goNext}
            className="px-6 py-2 rounded-full bg-red-600 text-sm font-medium hover:bg-red-700"
          >
            Next →
          </button>
        </div>
      </div>

      {/* Metadata panel */}
      {video && (
        <div className="mt-4">
          <div className="flex items-start gap-3">
            <Link
              href={`/channel/${video.channel_id}`}
              className="w-9 h-9 rounded-full bg-gradient-to-br from-red-500 to-purple-600 flex items-center justify-center text-sm font-bold shrink-0"
            >
              {(channel?.channelName || "?")[0].toUpperCase()}
            </Link>
            <div className="min-w-0 flex-1">
              <p className="text-lg font-medium leading-tight">{video.title}</p>
              <Link href={`/channel/${video.channel_id}`} className="text-sm text-neutral-400 hover:underline">
                {channel?.channelName || "…"}
              </Link>
              <p className="text-xs text-neutral-500 mt-0.5">
                {stats ? `${stats.views} views • ` : ""}
                {timeAgo(video.created_at)}
                {video.duration_seconds ? ` • ${formatDuration(video.duration_seconds)}` : ""}
              </p>
            </div>
            <Link
              href={`/watch/${video.video_id}`}
              className="shrink-0 text-sm border border-neutral-700 px-3 py-1.5 rounded hover:bg-neutral-800"
            >
              Open full page ↗
            </Link>
          </div>

          <div className="flex items-center gap-4 mt-3">
            <button onClick={() => handleReaction("like")} className="text-sm hover:text-red-400">
              👍 {stats?.likes ?? 0}
            </button>
            <button onClick={() => handleReaction("dislike")} className="text-sm hover:text-red-400">
              👎 {stats?.dislikes ?? 0}
            </button>
            {stats ? <span className="text-sm text-neutral-500">💬 {stats.comment_count}</span> : null}
          </div>

          {video.description && (
            <p className="text-sm text-neutral-400 mt-3 whitespace-pre-line">{video.description}</p>
          )}

          {(video.genre || (video.tags && video.tags.length > 0)) && (
            <div className="flex items-center gap-2 mt-2 flex-wrap">
              {video.genre && <span className="text-xs bg-neutral-800 px-2 py-0.5 rounded">{video.genre}</span>}
              {video.tags?.map((tag) => (
                <span key={tag} className="text-xs bg-neutral-800 text-neutral-300 px-2 py-0.5 rounded">
                  #{tag}
                </span>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
