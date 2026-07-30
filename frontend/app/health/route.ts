// Health endpoint for the ALB target-group health check (path standardized on /health across all
// services). Returns 200 so the frontend target group reports healthy. Distinct from /api/health,
// which the Route 53 region health check probes.
export function GET() {
  return new Response("ok");
}
