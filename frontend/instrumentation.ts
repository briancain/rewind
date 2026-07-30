import type { Instrumentation } from "next";

/**
 * Server-side error logging for CloudWatch.
 *
 * Next.js fires `onRequestError` for any uncaught error in SSR, route handlers, or server
 * components — all of which surface to the user as an HTTP 500. We emit a single flat JSON line
 * (with `status` at the top level) so the CloudWatch `frontend-5xx` metric filter
 * (`{ $.kubernetes.container_name = "frontend" && $.status >= 500 }`) can count frontend-origin 5xx.
 *
 * The Rust services log via the shared tracing JSON (status nested under `$.fields.status`); the
 * frontend owns its own shape here, so it logs `status` flat to match its filter.
 */
export const onRequestError: Instrumentation.onRequestError = (err, request) => {
  const message = err instanceof Error ? err.message : String(err);
  // One structured record per error, to stderr.
  console.error(
    JSON.stringify({
      level: "error",
      logger: "frontend",
      status: 500,
      path: request.path,
      method: request.method,
      message,
    }),
  );
};
