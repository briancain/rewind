import { attachWatchedHandler, attachEndedHandler, tryAutoplay } from "@/lib/player";

// A minimal fake media element: records listeners and lets tests emit events.
function makeEl(play?: () => Promise<void> | void) {
  const listeners: Record<string, Array<() => void>> = {};
  return {
    currentTime: 0,
    play: play ?? (() => Promise.resolve()),
    addEventListener(type: string, l: () => void) {
      (listeners[type] ||= []).push(l);
    },
    removeEventListener(type: string, l: () => void) {
      listeners[type] = (listeners[type] || []).filter((x) => x !== l);
    },
    emit(type: string) {
      (listeners[type] || []).slice().forEach((l) => l());
    },
    count(type: string) {
      return (listeners[type] || []).length;
    },
  };
}

describe("attachWatchedHandler", () => {
  it("fires once after crossing the threshold, then detaches", () => {
    const el = makeEl();
    const onWatched = jest.fn();
    attachWatchedHandler(el, onWatched);

    el.emit("timeupdate"); // currentTime 0 -> below threshold
    expect(onWatched).not.toHaveBeenCalled();

    el.currentTime = 5;
    el.emit("timeupdate");
    expect(onWatched).toHaveBeenCalledTimes(1);

    el.currentTime = 30;
    el.emit("timeupdate"); // already fired + detached
    expect(onWatched).toHaveBeenCalledTimes(1);
    expect(el.count("timeupdate")).toBe(0);
  });

  it("respects a custom threshold", () => {
    const el = makeEl();
    const onWatched = jest.fn();
    attachWatchedHandler(el, onWatched, 10);

    el.currentTime = 5;
    el.emit("timeupdate");
    expect(onWatched).not.toHaveBeenCalled();

    el.currentTime = 10;
    el.emit("timeupdate");
    expect(onWatched).toHaveBeenCalledTimes(1);
  });

  it("cleanup removes the listener", () => {
    const el = makeEl();
    const cleanup = attachWatchedHandler(el, jest.fn());
    expect(el.count("timeupdate")).toBe(1);
    cleanup();
    expect(el.count("timeupdate")).toBe(0);
  });
});

describe("attachEndedHandler", () => {
  it("fires onEnded when the media ends", () => {
    const el = makeEl();
    const onEnded = jest.fn();
    attachEndedHandler(el, onEnded);
    el.emit("ended");
    expect(onEnded).toHaveBeenCalledTimes(1);
  });

  it("cleanup removes the listener", () => {
    const el = makeEl();
    const onEnded = jest.fn();
    const cleanup = attachEndedHandler(el, onEnded);
    cleanup();
    el.emit("ended");
    expect(onEnded).not.toHaveBeenCalled();
    expect(el.count("ended")).toBe(0);
  });
});

describe("tryAutoplay", () => {
  it("calls play()", () => {
    const play = jest.fn(() => Promise.resolve());
    const el = makeEl(play);
    tryAutoplay(el);
    expect(play).toHaveBeenCalledTimes(1);
  });

  it("swallows a rejected play promise (autoplay blocked)", async () => {
    const play = jest.fn(() => Promise.reject(new Error("NotAllowedError")));
    const el = makeEl(play);
    expect(() => tryAutoplay(el)).not.toThrow();
    // Let the microtask queue flush; the rejection must be handled (no unhandled rejection).
    await Promise.resolve();
  });

  it("tolerates a void return (no promise)", () => {
    const play = jest.fn(() => undefined);
    const el = makeEl(play);
    expect(() => tryAutoplay(el)).not.toThrow();
  });
});
