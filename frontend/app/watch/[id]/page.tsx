"use client";
import { useEffect, useState, useCallback } from "react";
import { useParams, useRouter } from "next/navigation";
import dynamic from "next/dynamic";
import Link from "next/link";
import { svc } from "@/lib/api";
import { useAuth, useRequireAuth } from "@/lib/auth";
import { timeAgo } from "@/lib/format";

// Client-only: the player loads Media Chrome + hls-video-element web components, which register
// custom elements on import and must not execute during SSR.
const VideoPlayer = dynamic(() => import("@/components/VideoPlayer"), { ssr: false });

interface VideoMeta { video_id: string; title: string; description: string; channel_id: string; status: string; genre: string; tags: string[]; visibility: string; }
interface Stats { likes: number; dislikes: number; views: number; comment_count: number; }
interface Comment { comment_id: string; user_id: string; text: string; created_at: string; likes: number; }

export default function WatchPage() {
  const { id } = useParams<{ id: string }>();
  const { user } = useAuth();
  const requireAuth = useRequireAuth();
  const router = useRouter();
  const [meta, setMeta] = useState<VideoMeta | null>(null);
  const [stats, setStats] = useState<Stats | null>(null);
  const [comments, setComments] = useState<Comment[]>([]);
  const [commentText, setCommentText] = useState("");
  const [streamUrl, setStreamUrl] = useState("");
  const [posterUrl, setPosterUrl] = useState("");
  const [userNames, setUserNames] = useState<Record<string, string>>({});
  const [sortBy, setSortBy] = useState<"newest" | "likes">("newest");
  const [editing, setEditing] = useState(false);
  const [channelName, setChannelName] = useState("");
  const [editTitle, setEditTitle] = useState("");
  const [editDesc, setEditDesc] = useState("");
  const [editGenre, setEditGenre] = useState("");
  const [editTags, setEditTags] = useState("");
  const [editVisibility, setEditVisibility] = useState("public");

  const sortedComments = [...comments].sort((a, b) => {
    if (sortBy === "likes") return b.likes - a.likes;
    return b.created_at.localeCompare(a.created_at);
  });

  useEffect(() => {
    svc<VideoMeta>("catalog", `/videos/${id}`).then((m) => {
      setMeta(m);
      setEditTitle(m.title);
      setEditDesc(m.description);
      setEditGenre(m.genre || "");
      setEditTags((m.tags || []).join(", "));
      setEditVisibility(m.visibility || "public");
      svc<{ display_name: string }>("identity", `/users/${m.channel_id}`)
        .then((u) => setChannelName(u.display_name))
        .catch(() => {});
    }).catch(() => {});
    svc<Stats>("social", `/videos/${id}/stats`).then(setStats).catch(() => {});
    svc<{ comments: Comment[] }>("social", `/videos/${id}/comments`).then((r) => {
      setComments(r.comments);
      // Resolve display names
      const ids = [...new Set(r.comments.map((c) => c.user_id))];
      ids.forEach((uid) => {
        svc<{ display_name: string }>("identity", `/users/${uid}`)
          .then((u) => setUserNames((prev) => ({ ...prev, [uid]: u.display_name })))
          .catch(() => {});
      });
    }).catch(() => {});
    svc<{ url: string }>("streaming", `/videos/${id}/stream-url`)
      .then((r) => setStreamUrl(r.url))
      .catch(() => setStreamUrl(""));
    svc<{ url: string }>("streaming", `/videos/${id}/thumbnail-url`)
      .then((r) => setPosterUrl(r.url))
      .catch(() => {});
  }, [id]);

  // Count a view once playback passes 5s (deduped per session), and record watch history.
  // Invoked by VideoPlayer via onWatched.
  const handleWatched = useCallback(() => {
    const viewed = JSON.parse(sessionStorage.getItem("viewed") || "[]");
    if (!viewed.includes(id)) {
      svc("social", `/videos/${id}/view`, { method: "POST" }).catch(() => {});
      sessionStorage.setItem("viewed", JSON.stringify([...viewed, id]));
    }
    svc("social", `/videos/${id}/history`, { method: "POST" }).catch(() => {});
  }, [id]);

  async function handleReaction(type: "like" | "dislike") {
    if (!requireAuth("like this video")) return;
    await svc("social", `/videos/${id}/${type}`, { method: "POST" });
    const s = await svc<Stats>("social", `/videos/${id}/stats`);
    setStats(s);
  }

  async function handleComment(e: React.FormEvent) {
    e.preventDefault();
    if (!requireAuth("comment")) return;
    if (!commentText.trim()) return;
    await svc("social", `/videos/${id}/comments`, { method: "POST", body: JSON.stringify({ text: commentText }) });
    setCommentText("");
    const r = await svc<{ comments: Comment[] }>("social", `/videos/${id}/comments`);
    setComments(r.comments);
  }

  async function handleCommentLike(commentId: string, type: "like" | "dislike") {
    if (!requireAuth("like comments")) return;
    await svc("social", `/videos/${id}/comments/${commentId}/${type}`, { method: "POST" });
    const r = await svc<{ comments: Comment[] }>("social", `/videos/${id}/comments`);
    setComments(r.comments);
  }

  async function handleDeleteComment(commentId: string) {
    await svc("social", `/videos/${id}/comments/${commentId}`, { method: "DELETE" });
    const r = await svc<{ comments: Comment[] }>("social", `/videos/${id}/comments`);
    setComments(r.comments);
  }

  async function handleEdit(e: React.FormEvent) {
    e.preventDefault();
    const tags = editTags.split(",").map((t) => t.trim()).filter(Boolean);
    await svc("catalog", `/videos/${id}`, {
      method: "PUT",
      body: JSON.stringify({ title: editTitle, description: editDesc, genre: editGenre, tags, visibility: editVisibility }),
    });
    setMeta((m) => m ? { ...m, title: editTitle, description: editDesc, genre: editGenre, tags, visibility: editVisibility } : m);
    setEditing(false);
  }

  if (!meta) return <p className="text-neutral-400">Loading...</p>;

  if (meta.visibility === "private" && (!user || user.user_id !== meta.channel_id)) {
    return (
      <div className="max-w-4xl mx-auto flex flex-col items-center justify-center gap-3 py-20">
        <div className="text-4xl">🔒</div>
        <p className="text-neutral-200 text-lg font-medium">This video is private</p>
        <p className="text-neutral-500 text-sm">The owner has restricted access to this video.</p>
      </div>
    );
  }

  const isFailed = meta.status === "failed";
  const isProcessing =
    !isFailed && (!streamUrl || meta.status === "pending" || meta.status === "processing");

  return (
    <div className="max-w-4xl mx-auto">
      {isFailed ? (
        <div className="w-full aspect-video bg-neutral-900 rounded-lg flex flex-col items-center justify-center gap-3">
          <div className="text-4xl">⚠️</div>
          <p className="text-neutral-300 text-lg">Processing failed</p>
          <p className="text-neutral-500 text-sm">This video couldn&apos;t be transcoded. Try re-uploading it.</p>
        </div>
      ) : isProcessing ? (
        <div className="w-full aspect-video bg-neutral-900 rounded-lg flex flex-col items-center justify-center gap-3">
          <div className="text-4xl">⏳</div>
          <p className="text-neutral-400 text-lg">Video is processing...</p>
          <p className="text-neutral-500 text-sm">This may take a few minutes. Refresh to check status.</p>
        </div>
      ) : (
        <VideoPlayer streamUrl={streamUrl} posterUrl={posterUrl || undefined} onWatched={handleWatched} />
      )}
      <h1 className="text-xl font-bold mt-4">{meta.title}</h1>
      <div className="flex items-center gap-3 mt-3">
        <Link href={`/channel/${meta.channel_id}`} className="w-9 h-9 rounded-full bg-gradient-to-br from-red-500 to-purple-600 flex items-center justify-center text-sm font-bold shrink-0">
          {(channelName || "?")[0].toUpperCase()}
        </Link>
        <Link href={`/channel/${meta.channel_id}`} className="text-sm font-medium hover:underline">{channelName || "..."}</Link>
        <button
          onClick={() => alert("🚧 Subscriptions coming soon!")}
          className="ml-auto text-sm bg-red-600 text-white px-4 py-1.5 rounded-full font-medium hover:bg-red-700 transition"
        >
          Subscribe
        </button>
      </div>
      <p className="text-sm text-neutral-400 mt-1">{meta.description}</p>
      <div className="flex items-center gap-2 mt-2 flex-wrap">
        {meta.genre && <span className="text-xs bg-neutral-800 px-2 py-0.5 rounded">{meta.genre}</span>}
        {meta.tags?.map((tag) => (
          <span key={tag} className="text-xs bg-neutral-800 text-neutral-300 px-2 py-0.5 rounded">#{tag}</span>
        ))}
      </div>

      {user && user.user_id === meta.channel_id && (
        <div className="flex items-center gap-3 mt-3">
          <button
            onClick={() => setEditing(!editing)}
            className="text-sm border border-neutral-600 px-3 py-1 rounded hover:bg-neutral-800"
          >
            {editing ? "Cancel" : "Edit"}
          </button>
          <button
            onClick={async () => {
              if (!confirm("Delete this video?")) return;
              await svc("catalog", `/videos/${id}`, { method: "DELETE" });
              router.push("/");
            }}
            className="text-sm border border-red-800 text-red-400 px-3 py-1 rounded hover:bg-red-950"
          >
            Delete
          </button>
        </div>
      )}

      {editing && (
        <form onSubmit={handleEdit} className="mt-4 space-y-3 max-w-lg">
          <input value={editTitle} onChange={(e) => setEditTitle(e.target.value)} placeholder="Title" className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700 text-sm" />
          <textarea value={editDesc} onChange={(e) => setEditDesc(e.target.value)} placeholder="Description" className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700 text-sm h-20" />
          <input value={editGenre} onChange={(e) => setEditGenre(e.target.value)} placeholder="Genre" className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700 text-sm" />
          <input value={editTags} onChange={(e) => setEditTags(e.target.value)} placeholder="Tags (comma separated)" className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700 text-sm" />
          <select value={editVisibility} onChange={(e) => setEditVisibility(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700 text-sm">
            <option value="public">Public — visible to everyone</option>
            <option value="unlisted">Unlisted — only accessible via direct link</option>
            <option value="private">Private — only you can see this</option>
          </select>
          <button type="submit" className="px-4 py-1.5 bg-red-600 rounded text-sm hover:bg-red-700">Save</button>
        </form>
      )}

      {stats && (
        <div className="flex items-center gap-4 mt-3">
          {user && user.user_id === meta.channel_id && meta.visibility === "private" && (
            <span className="text-sm bg-red-900/50 text-red-300 px-2 py-0.5 rounded">🔒 Private</span>
          )}
          {user && user.user_id === meta.channel_id && meta.visibility === "unlisted" && (
            <span className="text-sm bg-yellow-900/50 text-yellow-300 px-2 py-0.5 rounded">🔗 Unlisted</span>
          )}
          <span className="text-sm text-neutral-400">{stats.views} views</span>
          <button onClick={() => handleReaction("like")} className="text-sm hover:text-red-400">👍 {stats.likes}</button>
          <button onClick={() => handleReaction("dislike")} className="text-sm hover:text-red-400">👎 {stats.dislikes}</button>
        </div>
      )}

      <div className="mt-8">
        <div className="flex items-center justify-between mb-3">
          <h2 className="font-bold">Comments ({comments.length})</h2>
          <select
            value={sortBy}
            onChange={(e) => setSortBy(e.target.value as "newest" | "likes")}
            className="text-sm bg-neutral-800 border border-neutral-700 rounded px-2 py-1"
          >
            <option value="newest">Newest</option>
            <option value="likes">Most liked</option>
          </select>
        </div>
        {user && (
          <form onSubmit={handleComment} className="flex gap-2 mb-4">
            <input value={commentText} onChange={(e) => setCommentText(e.target.value)} placeholder="Add a comment..." className="flex-1 px-3 py-1.5 rounded bg-neutral-800 border border-neutral-700 text-sm" />
            <button type="submit" className="px-3 py-1.5 bg-red-600 rounded text-sm">Post</button>
          </form>
        )}
        <div className="space-y-3">
          {sortedComments.map((c) => (
            <div key={c.comment_id} className="bg-neutral-900 p-3 rounded">
              <div className="flex items-center gap-2">
                <Link href={`/channel/${c.user_id}`} className="text-sm font-medium text-red-300 hover:underline">
                  {userNames[c.user_id] || (user && c.user_id === user.user_id ? user.display_name : c.user_id)}
                </Link>
                <span className="text-xs text-neutral-500">{timeAgo(c.created_at)}</span>
              </div>
              <p className="text-sm mt-1">{c.text}</p>
              <div className="flex items-center gap-3 mt-2">
                <button
                  onClick={() => handleCommentLike(c.comment_id, "like")}
                  className="text-xs text-neutral-400 hover:text-white"
                >👍 {c.likes > 0 ? c.likes : ""}</button>
                <button
                  onClick={() => handleCommentLike(c.comment_id, "dislike")}
                  className="text-xs text-neutral-400 hover:text-white"
                >👎 {c.likes < 0 ? Math.abs(c.likes) : ""}</button>
                {user && c.user_id === user.user_id && (
                  <button
                    onClick={() => handleDeleteComment(c.comment_id)}
                    className="text-xs text-neutral-500 hover:text-red-400 ml-auto"
                  >Delete</button>
                )}
              </div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
