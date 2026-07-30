// Pure media-element wiring helpers, extracted from VideoPlayer so the new playback behavior
// (view counting, end-of-video auto-advance, gesture-backed autoplay) is unit-testable without
// rendering the Media Chrome / <hls-video> web components (which register custom elements on
// import and don't run cleanly under jsdom).

/** The minimal slice of an HTMLMediaElement we depend on — lets tests pass a fake element. */
export interface MediaLike {
  currentTime: number;
  addEventListener(type: string, listener: () => void): void;
  removeEventListener(type: string, listener: () => void): void;
  play(): Promise<void> | void;
}

/**
 * Fire `onWatched` once when playback first passes `threshold` seconds, then detach.
 * Returns a cleanup function that removes the listener.
 */
export function attachWatchedHandler(
  el: MediaLike,
  onWatched: () => void,
  threshold = 5,
): () => void {
  let fired = false;
  const onTime = () => {
    if (!fired && el.currentTime >= threshold) {
      fired = true;
      onWatched();
      el.removeEventListener("timeupdate", onTime);
    }
  };
  el.addEventListener("timeupdate", onTime);
  return () => el.removeEventListener("timeupdate", onTime);
}

/**
 * Fire `onEnded` when the media reaches its natural end (used to auto-flip to the next surf
 * channel). Returns a cleanup function that removes the listener.
 */
export function attachEndedHandler(el: MediaLike, onEnded: () => void): () => void {
  const onEnd = () => onEnded();
  el.addEventListener("ended", onEnd);
  return () => el.removeEventListener("ended", onEnd);
}

/**
 * Best-effort autoplay. `play()` returns a promise that rejects if the browser blocks autoplay
 * (no user activation) or if a rapid source swap interrupts the load; both are non-fatal here
 * (the surf "power on" click and Next/Back clicks supply activation), so swallow the rejection.
 */
export function tryAutoplay(el: MediaLike): void {
  const result = el.play();
  if (result && typeof (result as Promise<void>).catch === "function") {
    (result as Promise<void>).catch(() => {});
  }
}
