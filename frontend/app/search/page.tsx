"use client";
import { useEffect, useState, Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { svc } from "@/lib/api";
import { formatDuration, timeAgo } from "@/lib/format";
import { resolveSearchView } from "@/lib/search";
import { Thumbnail } from "@/components/Thumbnail";
import Link from "next/link";

interface VideoResult { video_id: string; title: string; description: string; channel_id: string; }

function SearchResultCard({ video }: { video: VideoResult }) {
  const [channelName, setChannelName] = useState("");
  const [views, setViews] = useState<number | null>(null);
  const [createdAt, setCreatedAt] = useState("");
  const [duration, setDuration] = useState<number | null>(null);

  useEffect(() => {
    svc<{ display_name: string }>("identity", `/users/${video.channel_id}`)
      .then((u) => setChannelName(u.display_name))
      .catch(() => {});
    svc<{ views: number }>("social", `/videos/${video.video_id}/stats`)
      .then((s) => setViews(s.views))
      .catch(() => {});
    svc<{ created_at: string; duration_seconds?: number }>("catalog", `/videos/${video.video_id}`)
      .then((v) => { setCreatedAt(v.created_at); if (v.duration_seconds) setDuration(v.duration_seconds); })
      .catch(() => {});
  }, [video.channel_id, video.video_id]);

  return (
    <Link href={`/watch/${video.video_id}`} className="flex gap-4 bg-neutral-900 rounded-lg p-3 hover:ring-1 hover:ring-red-500">
      <div className="w-48 aspect-video bg-neutral-800 rounded shrink-0 overflow-hidden relative">
        <Thumbnail videoId={video.video_id} iconSize="text-2xl" />
        {duration && (
          <span className="absolute bottom-1 right-1 bg-black/80 text-white text-xs px-1 rounded">
            {formatDuration(duration)}
          </span>
        )}
      </div>
      <div>
        <p className="font-medium">{video.title}</p>
        <p className="text-sm text-neutral-400 mt-1">{channelName || "..."}</p>
        <p className="text-xs text-neutral-500 mt-0.5">
          {views !== null ? `${views} views` : ""}{views !== null && createdAt && " • "}{createdAt && timeAgo(createdAt)}
        </p>
        <p className="text-sm text-neutral-500 mt-1 line-clamp-2">{video.description}</p>
      </div>
    </Link>
  );
}

function SearchResults() {
  const searchParams = useSearchParams();
  const view = resolveSearchView(searchParams.get("q"), searchParams.get("tag"));
  const [results, setResults] = useState<VideoResult[]>([]);
  const [total, setTotal] = useState(0);

  useEffect(() => {
    if (!view.path) return;
    setResults([]);
    setTotal(0);
    svc<{ results: VideoResult[]; total: number }>("search", view.path)
      .then((r) => { setResults(r.results); setTotal(r.total); })
      .catch(() => {});
  }, [view.path]);

  if (view.mode === "empty") {
    return <p className="text-neutral-400">Enter a search term to find videos.</p>;
  }

  return (
    <div>
      <h1 className="text-2xl font-bold mb-2">{view.heading}</h1>
      <p className="text-sm text-neutral-400 mb-6">{total} results</p>
      <div className="space-y-4">
        {results.map((v) => (
          <SearchResultCard key={v.video_id} video={v} />
        ))}
      </div>
      {results.length === 0 && <p className="text-neutral-400">{view.emptyMessage}</p>}
    </div>
  );
}

export default function SearchPage() {
  return (
    <Suspense fallback={<p className="text-neutral-400">Loading...</p>}>
      <SearchResults />
    </Suspense>
  );
}
