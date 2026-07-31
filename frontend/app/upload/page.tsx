"use client";
import { useState } from "react";
import { svc } from "@/lib/api";
import { useAuth } from "@/lib/auth";
import { useRouter } from "next/navigation";

export default function UploadPage() {
  const { user } = useAuth();
  const router = useRouter();
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [tags, setTags] = useState("");
  const [genre, setGenre] = useState("General");
  const [file, setFile] = useState<File | null>(null);
  const [progress, setProgress] = useState(0);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState("");

  if (!user) return <p className="text-neutral-400 mt-10">Please login to upload.</p>;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!file) return;
    setUploading(true);
    setError("");

    try {
      // 1. Create video in catalog
      const video = await svc<{ video_id: string }>("catalog", "/videos", {
        method: "POST",
        body: JSON.stringify({ title, description, genre, tags: tags.split(",").map((t) => t.trim()).filter(Boolean) }),
      });

      // 2. Initiate multipart upload
      const partSize = 5 * 1024 * 1024;
      const partCount = Math.ceil(file.size / partSize);
      const initRes = await svc<{ upload_id: string; s3_key: string; presigned_urls: string[] }>("upload", "/uploads/initiate", {
        method: "POST",
        body: JSON.stringify({ video_id: video.video_id, filename: file.name, content_type: file.type || "video/mp4", part_count: partCount }),
      });

      // 3. Upload parts directly to S3
      for (let i = 0; i < partCount; i++) {
        const chunk = file.slice(i * partSize, (i + 1) * partSize);
        const resp = await fetch(initRes.presigned_urls[i], { method: "PUT", body: chunk });
        if (!resp.ok) throw new Error(`Part ${i + 1} upload failed (${resp.status})`);
        setProgress(Math.round(((i + 1) / partCount) * 100));
      }

      // 4. Complete upload (server assembles parts via S3 ListParts)
      await svc("upload", "/uploads/complete", {
        method: "POST",
        body: JSON.stringify({ video_id: video.video_id, upload_id: initRes.upload_id, s3_key: initRes.s3_key }),
      });

      router.push(`/watch/${video.video_id}`);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Upload failed");
    } finally {
      setUploading(false);
    }
  }

  return (
    <div className="max-w-lg mx-auto mt-10">
      <h1 className="text-2xl font-bold mb-6">Upload Video</h1>
      {error && <p className="text-red-400 mb-4">{error}</p>}
      <form onSubmit={handleSubmit} className="space-y-4">
        <input type="text" placeholder="Title" value={title} onChange={(e) => setTitle(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700" required />
        <textarea placeholder="Description" value={description} onChange={(e) => setDescription(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700 h-24" />
        <input type="text" placeholder="Tags (comma separated)" value={tags} onChange={(e) => setTags(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700" />
        <input type="text" placeholder="Genre" value={genre} onChange={(e) => setGenre(e.target.value)} className="w-full px-3 py-2 rounded bg-neutral-800 border border-neutral-700" />
        <label className="block w-full border-2 border-dashed border-neutral-700 rounded-lg p-6 text-center cursor-pointer hover:border-red-500 transition">
          <span className="text-neutral-400">{file ? file.name : "Click to select a video file"}</span>
          <input type="file" accept="video/*" onChange={(e) => setFile(e.target.files?.[0] || null)} className="hidden" required />
        </label>
        {uploading && (
          <div className="w-full bg-neutral-800 rounded h-2">
            <div className="bg-red-600 h-2 rounded transition-all" style={{ width: `${progress}%` }} />
          </div>
        )}
        <button type="submit" disabled={uploading} className="w-full py-2 bg-red-600 rounded hover:bg-red-700 disabled:opacity-50">
          {uploading ? `Uploading ${progress}%` : "Upload"}
        </button>
      </form>
    </div>
  );
}
