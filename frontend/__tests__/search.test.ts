import { resolveSearchView } from "@/lib/search";

describe("resolveSearchView", () => {
  it("resolves free-text mode from q", () => {
    const v = resolveSearchView("cats", null);
    expect(v.mode).toBe("text");
    expect(v.path).toBe("/search?q=cats");
    expect(v.heading).toBe('Search: "cats"');
    expect(v.emptyMessage).toBe("No results found.");
  });

  it("resolves tag mode from tag", () => {
    const v = resolveSearchView(null, "cats");
    expect(v.mode).toBe("tag");
    expect(v.path).toBe("/search?tag=cats");
    expect(v.heading).toBe("#cats");
    expect(v.emptyMessage).toBe("No videos tagged #cats yet.");
  });

  it("prefers tag over q when both are present", () => {
    const v = resolveSearchView("something", "cats");
    expect(v.mode).toBe("tag");
    expect(v.path).toBe("/search?tag=cats");
  });

  it("url-encodes multi-word / special-char tags", () => {
    const v = resolveSearchView(null, "big cats & dogs");
    expect(v.path).toBe("/search?tag=big%20cats%20%26%20dogs");
    expect(v.heading).toBe("#big cats & dogs");
  });

  it("url-encodes free-text terms", () => {
    const v = resolveSearchView("rust programming", null);
    expect(v.path).toBe("/search?q=rust%20programming");
  });

  it("trims whitespace and treats blank as empty", () => {
    expect(resolveSearchView("   ", null).mode).toBe("empty");
    expect(resolveSearchView(null, "  ").mode).toBe("empty");
    expect(resolveSearchView("", "").path).toBeNull();
  });

  it("trims surrounding whitespace on a real term", () => {
    const v = resolveSearchView(null, "  cats  ");
    expect(v.mode).toBe("tag");
    expect(v.path).toBe("/search?tag=cats");
    expect(v.heading).toBe("#cats");
  });

  it("returns empty view when nothing is provided", () => {
    const v = resolveSearchView(null, null);
    expect(v.mode).toBe("empty");
    expect(v.path).toBeNull();
    expect(v.heading).toBe("");
  });
});
