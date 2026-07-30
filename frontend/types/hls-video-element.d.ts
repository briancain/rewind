import type { DetailedHTMLProps, VideoHTMLAttributes } from "react";

// `<hls-video>` (from `hls-video-element`) is a custom element that behaves like a native
// `<video>` — it wraps hls.js and exposes the standard media + videoRenditions APIs that
// Media Chrome's controls bind to. Declare it for JSX so TSX usage type-checks. It accepts the
// standard video attributes (src, poster, crossOrigin, playsInline, muted, slot, ref, ...).
declare module "react" {
  namespace JSX {
    interface IntrinsicElements {
      "hls-video": DetailedHTMLProps<
        VideoHTMLAttributes<HTMLVideoElement>,
        HTMLVideoElement
      >;
    }
  }
}
