// Pure logic for the search page: turn the URL query params (`q` free-text, `tag` exact hashtag)
// into the backend request path and the page's display strings. Kept free of React so it
// unit-tests directly (see __tests__/search.test.ts), matching the codebase's lib/*.ts pattern.
//
// Tag mode (a clicked hashtag) takes precedence over free-text and hits the same `/search` endpoint
// with `?tag=` instead of `?q=` — the backend then runs an exact, newest-first tag filter.

export type SearchMode = "tag" | "text" | "empty";

export interface SearchView {
  mode: SearchMode;
  /// Backend request path, or null when there is nothing to query.
  path: string | null;
  /// Page heading.
  heading: string;
  /// Message shown when a query runs but returns nothing.
  emptyMessage: string;
}

/**
 * Resolve the search-page view from the raw `q` and `tag` query params.
 * `tag` wins when present; both are trimmed, and blank values fall through to "empty" (no request).
 */
export function resolveSearchView(
  q: string | null | undefined,
  tag: string | null | undefined
): SearchView {
  const tagTerm = (tag ?? "").trim();
  if (tagTerm) {
    return {
      mode: "tag",
      path: `/search?tag=${encodeURIComponent(tagTerm)}`,
      heading: `#${tagTerm}`,
      emptyMessage: `No videos tagged #${tagTerm} yet.`,
    };
  }

  const textTerm = (q ?? "").trim();
  if (textTerm) {
    return {
      mode: "text",
      path: `/search?q=${encodeURIComponent(textTerm)}`,
      heading: `Search: "${textTerm}"`,
      emptyMessage: "No results found.",
    };
  }

  return { mode: "empty", path: null, heading: "", emptyMessage: "" };
}
