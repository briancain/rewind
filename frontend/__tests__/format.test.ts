import { formatDuration, timeAgo } from "@/lib/format";

describe("formatDuration", () => {
  it("formats seconds under a minute", () => {
    expect(formatDuration(0)).toBe("0:00");
    expect(formatDuration(5)).toBe("0:05");
    expect(formatDuration(59)).toBe("0:59");
  });

  it("formats minutes and seconds", () => {
    expect(formatDuration(60)).toBe("1:00");
    expect(formatDuration(90)).toBe("1:30");
    expect(formatDuration(125)).toBe("2:05");
  });

  it("formats longer durations", () => {
    expect(formatDuration(600)).toBe("10:00");
    expect(formatDuration(3661)).toBe("61:01");
  });

  it("truncates fractional seconds", () => {
    expect(formatDuration(90.7)).toBe("1:30");
    expect(formatDuration(5.9)).toBe("0:05");
  });
});

describe("timeAgo", () => {
  it("returns 'just now' for recent timestamps", () => {
    const now = new Date().toISOString();
    expect(timeAgo(now)).toBe("just now");
  });

  it("returns minutes ago", () => {
    const date = new Date(Date.now() - 5 * 60 * 1000).toISOString();
    expect(timeAgo(date)).toBe("5m ago");
  });

  it("returns hours ago", () => {
    const date = new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString();
    expect(timeAgo(date)).toBe("3h ago");
  });

  it("returns days ago", () => {
    const date = new Date(Date.now() - 7 * 24 * 60 * 60 * 1000).toISOString();
    expect(timeAgo(date)).toBe("7d ago");
  });

  it("returns formatted date for old timestamps", () => {
    const date = new Date(Date.now() - 60 * 24 * 60 * 60 * 1000).toISOString();
    expect(timeAgo(date)).toMatch(/\d{1,2}\/\d{1,2}\/\d{4}/);
  });
});
