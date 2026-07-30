"use client";
import { useEffect, useState } from "react";
import { svc } from "@/lib/api";
import { timeAgo } from "@/lib/format";
import { Thumbnail } from "@/components/Thumbnail";
import { useAuth } from "@/lib/auth";
import Link from "next/link";
import { useRouter } from "next/navigation";

interface HistoryEntry { video_id: string; watched_at: string; }
interface VideoInfo { title: string; channel_id: string; duration_seconds?: number; }

export default function HistoryPage() {
  const { user } = useAuth();
  const router = useRouter();
  const [entries, setEntries] = useState<(HistoryEntry & Partial<VideoInfo>)[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    if (!user) { router.push("/login"); return; }
    svc<{ entries: HistoryEntry[] }>("social", "/history")
      .then(async (r) => {
        const enriched = await Promise.all(
          r.entries.map(async (e) => {
            const info = await svc<VideoInfo>("catalog", `/videos/${e.video_id}`).catch(() => null);
            return { ...e, ...info };
          })
        );
        setEntries(enriched);
      })
      .catch(() => {})
      .finally(() => setLoading(false));
  }, [user, router]);

  async function removeEntry(watched_at: string) {
    await svc("social", `/history?watched_at=${encodeURIComponent(watched_at)}`, { method: "DELETE" });
    setEntries((prev) => prev.filter((e) => e.watched_at !== watched_at));
  }

  if (!user) return null;
  if (loading) return <p className="text-neutral-400">Loading...</p>;

  return (
    <div>
      <h1 className="text-2xl font-bold mb-6">Watch History</h1>
      {entries.length === 0 ? (
        <p className="text-neutral-400">No watch history yet.</p>
      ) : (
        <div className="space-y-3">
          {entries.map((e) => (
            <div key={e.watched_at} className="flex gap-4 bg-neutral-900 rounded-lg p-3 items-center">
              <Link href={`/watch/${e.video_id}`} className="w-48 aspect-video bg-neutral-800 rounded shrink-0 overflow-hidden relative block">
                <Thumbnail videoId={e.video_id} iconSize="text-2xl" />
              </Link>
              <div className="flex-1 min-w-0">
                <Link href={`/watch/${e.video_id}`} className="font-medium truncate block hover:text-red-400">
                  {e.title || e.video_id}
                </Link>
                <p className="text-xs text-neutral-500">Watched {timeAgo(e.watched_at)}</p>
              </div>
              <button
                onClick={() => removeEntry(e.watched_at)}
                className="text-xs text-neutral-500 hover:text-red-400 shrink-0"
              >✕ Remove</button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
