"use client";
import { useEffect, useState } from "react";
import { svc } from "@/lib/api";
import { formatDuration, timeAgo } from "@/lib/format";
import { Thumbnail } from "@/components/Thumbnail";
import Link from "next/link";

interface Video {
  video_id: string;
  title: string;
  channel_id: string;
  status: string;
  created_at: string;
  duration_seconds?: number;
}

function VideoCard({ video }: { video: Video }) {
  const [channelName, setChannelName] = useState("");
  const [views, setViews] = useState<number | null>(null);

  useEffect(() => {
    svc<{ display_name: string }>("identity", `/users/${video.channel_id}`)
      .then((u) => setChannelName(u.display_name))
      .catch(() => {});
    svc<{ views: number }>("social", `/videos/${video.video_id}/stats`)
      .then((s) => setViews(s.views))
      .catch(() => {});
  }, [video.channel_id, video.video_id]);

  return (
    <Link href={`/watch/${video.video_id}`} className="bg-neutral-900 rounded-lg overflow-hidden hover:ring-1 hover:ring-red-500 transition">
      <div className="aspect-video bg-neutral-800 relative">
        <Thumbnail videoId={video.video_id} />
        {video.duration_seconds && (
          <span className="absolute bottom-1 right-1 bg-black/80 text-white text-xs px-1 rounded">
            {formatDuration(video.duration_seconds)}
          </span>
        )}
      </div>
      <div className="p-3">
        <p className="font-medium truncate">{video.title}</p>
        <p className="text-sm text-neutral-400">{channelName || "..."}</p>
        <p className="text-xs text-neutral-500">
          {views !== null ? `${views} views` : ""}{views !== null && " • "}{timeAgo(video.created_at)}
        </p>
      </div>
    </Link>
  );
}

export default function HomePage() {
  const [videos, setVideos] = useState<Video[]>([]);

  useEffect(() => {
    svc<{ videos: Video[] }>("catalog", "/videos/feed")
      .then((r) => setVideos(r.videos))
      .catch(() => {});
  }, []);

  return (
    <div>
      <h1 className="text-2xl font-bold mb-6">Browse</h1>
      {videos.length === 0 ? (
        <p className="text-neutral-400">No videos yet.</p>
      ) : (
        <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4">
          {videos.map((v) => (
            <VideoCard key={v.video_id} video={v} />
          ))}
        </div>
      )}
    </div>
  );
}
