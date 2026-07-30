"use client";
import { useEffect, useState } from "react";
import { svc } from "@/lib/api";

export function Thumbnail({ videoId, iconSize = "text-4xl" }: { videoId: string; iconSize?: string }) {
  const [src, setSrc] = useState("");
  useEffect(() => {
    svc<{ url: string }>("streaming", `/videos/${videoId}/thumbnail-url`)
      .then((r) => setSrc(r.url))
      .catch(() => {});
  }, [videoId]);

  if (!src) return <div className="w-full h-full flex items-center justify-center"><span className={iconSize}>▶</span></div>;
  return <img src={src} alt="" className="w-full h-full object-cover" />;
}
