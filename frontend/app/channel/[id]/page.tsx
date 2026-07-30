"use client";
import { useEffect, useState } from "react";
import { useParams } from "next/navigation";
import { svc } from "@/lib/api";
import { formatDuration } from "@/lib/format";
import { Thumbnail } from "@/components/Thumbnail";
import { useAuth } from "@/lib/auth";
import Link from "next/link";

interface Video { video_id: string; title: string; status: string; visibility?: string; duration_seconds?: number; }

export default function ChannelPage() {
  const { id } = useParams<{ id: string }>();
  const { user } = useAuth();
  const [videos, setVideos] = useState<Video[]>([]);

  useEffect(() => {
    svc<{ videos: Video[] }>("catalog", `/videos?channel_id=${id}`)
      .then((r) => setVideos(r.videos))
      .catch(() => {});
  }, [id]);

  const isOwner = user?.user_id === id;

  return (
    <div>
      <h1 className="text-2xl font-bold mb-6">{isOwner ? "My Channel" : "Channel"}</h1>
      <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4">
        {videos.map((v) => (
          <Link key={v.video_id} href={`/watch/${v.video_id}`} className="bg-neutral-900 rounded-lg overflow-hidden hover:ring-1 hover:ring-red-500">
            <div className="aspect-video bg-neutral-800 relative">
              <Thumbnail videoId={v.video_id} />
              {v.duration_seconds && (
                <span className="absolute bottom-1 right-1 bg-black/80 text-white text-xs px-1 rounded">
                  {formatDuration(v.duration_seconds)}
                </span>
              )}
              {isOwner && v.visibility === "private" && (
                <span className="absolute top-1 left-1 bg-red-900/80 text-red-200 text-xs px-1.5 py-0.5 rounded">🔒 Private</span>
              )}
              {isOwner && v.visibility === "unlisted" && (
                <span className="absolute top-1 left-1 bg-yellow-900/80 text-yellow-200 text-xs px-1.5 py-0.5 rounded">🔗 Unlisted</span>
              )}
            </div>
            <div className="p-3">
              <p className="font-medium truncate">{v.title}</p>
              {v.status !== "published" && (
                <span className="text-xs bg-neutral-700 px-2 py-0.5 rounded mt-1 inline-block">{v.status}</span>
              )}
            </div>
          </Link>
        ))}
      </div>
      {videos.length === 0 && <p className="text-neutral-400">No videos on this channel.</p>}
    </div>
  );
}
