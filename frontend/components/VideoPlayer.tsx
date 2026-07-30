"use client";
// Registers the <hls-video> custom element (wraps hls.js, exposes renditions to Media Chrome).
// This module only ever loads on the client — the watch page imports VideoPlayer via
// next/dynamic({ ssr: false }) — so customElements.define is never called during SSR.
import "hls-video-element";
import { useEffect, useRef } from "react";
import {
  MediaController,
  MediaControlBar,
  MediaPlayButton,
  MediaSeekBackwardButton,
  MediaSeekForwardButton,
  MediaTimeRange,
  MediaTimeDisplay,
  MediaMuteButton,
  MediaVolumeRange,
  MediaPipButton,
  MediaFullscreenButton,
  MediaPosterImage,
  MediaLoadingIndicator,
} from "media-chrome/react";
import { MediaRenditionMenu, MediaRenditionMenuButton } from "media-chrome/react/menu";
import { attachWatchedHandler, attachEndedHandler, tryAutoplay } from "@/lib/player";

interface VideoPlayerProps {
  /** HLS master manifest (.m3u8) for public/unlisted, or a presigned MP4 for private videos. */
  streamUrl: string;
  posterUrl?: string;
  /** Fired once after 5s of playback (used for view-count + watch history). */
  onWatched?: () => void;
  /**
   * Start playback automatically on mount and whenever `streamUrl` changes. Used by the surf
   * "TV" experience, where the user's "turn on" / Next / Back clicks supply the activation the
   * browser autoplay policy requires. The watch page omits this (manual play).
   */
  autoPlay?: boolean;
  /** Fired when the video plays to its natural end — surf uses this to auto-flip to the next channel. */
  onEnded?: () => void;
}

// Map Media Chrome's CSS variables to the app's dark/red theme. Unknown vars are harmless no-ops.
const THEME = {
  "--media-primary-color": "rgb(245 245 245)", // icons + text
  "--media-range-bar-color": "rgb(220 38 38)", // played progress / volume fill (red-600)
  "--media-menu-background": "rgb(23 23 23 / 0.95)", // neutral-900 menu surface
  "--media-menu-item-checked-background": "rgb(220 38 38 / 0.3)",
} as React.CSSProperties;

/**
 * In-player video UI built on Media Chrome. HLS streams play via <hls-video> (hls.js under the
 * hood, native HLS on Safari) and expose an ABR rendition menu — the ⚙ quality gear lives in the
 * control bar. Private videos arrive as a presigned MP4 and render a plain <video> (the rendition
 * menu auto-hides when there are no renditions).
 */
export default function VideoPlayer({ streamUrl, posterUrl, onWatched, autoPlay, onEnded }: VideoPlayerProps) {
  const mediaRef = useRef<HTMLVideoElement | null>(null);
  const isHls = streamUrl.endsWith(".m3u8");

  // Count a view once playback passes 5s. Both <hls-video> and <video> are real media elements,
  // so the timeupdate listener is identical regardless of source type. Re-runs on source change
  // (a new surf channel) so each channel counts its own view.
  useEffect(() => {
    const el = mediaRef.current;
    if (!el || !onWatched) return;
    return attachWatchedHandler(el, onWatched);
  }, [streamUrl, onWatched]);

  // Auto-advance: fire onEnded when the clip finishes (surf flips to the next channel).
  useEffect(() => {
    const el = mediaRef.current;
    if (!el || !onEnded) return;
    return attachEndedHandler(el, onEnded);
  }, [streamUrl, onEnded]);

  // Gesture-backed autoplay on mount and on every channel flip (source swap).
  useEffect(() => {
    const el = mediaRef.current;
    if (!el || !autoPlay) return;
    tryAutoplay(el);
  }, [streamUrl, autoPlay]);

  return (
    <MediaController
      className="w-full aspect-video bg-black rounded-lg overflow-hidden"
      style={THEME}
    >
      {isHls ? (
        <hls-video ref={mediaRef} slot="media" src={streamUrl} crossOrigin="" playsInline />
      ) : (
        <video ref={mediaRef} slot="media" src={streamUrl} crossOrigin="" playsInline />
      )}
      {posterUrl ? <MediaPosterImage slot="poster" src={posterUrl} /> : null}
      <MediaLoadingIndicator slot="centered-chrome" noAutohide />
      {/* The menu must come BEFORE the control bar. Both render into the controller's bottom
          region; a closed menu still reserves layout space, so placing it after the control bar
          pushes the bar up off the bottom. Before it, the (invisible, click-through) reserved space
          sits above the bottom-pinned bar and the menu pops up into it. */}
      <MediaRenditionMenu hidden anchor="auto" />
      <MediaControlBar>
        <MediaPlayButton />
        <MediaSeekBackwardButton seekOffset={10} />
        <MediaSeekForwardButton seekOffset={10} />
        <MediaTimeRange />
        <MediaTimeDisplay showDuration />
        <MediaMuteButton />
        <MediaVolumeRange />
        <MediaRenditionMenuButton />
        <MediaPipButton />
        <MediaFullscreenButton />
      </MediaControlBar>
    </MediaController>
  );
}
