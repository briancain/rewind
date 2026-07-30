# Rewind — Tasks & Backlog

The living task list for Rewind. **`DESIGN.md` describes the system as-built (timeless); this file
tracks work that isn't done yet.**

**Conventions (keep this file bounded):**
- Only **open** work lives here. When a task ships, fold the design into `DESIGN.md` (present-tense,
  as-built) and **delete the task from this file** — git history is the record of what/when.
- Don't track completed milestones or dates here or in `DESIGN.md`; that's what `git log` is for.
- New work starts as a task here, not as a new section in `DESIGN.md`.

---

## This round

Nothing in flight. The cascade-delete story is complete end-to-end.
(Shipped: WAF, the thumbnails-bucket cleanup, `streaming`'s shared S3 client, the typed-error
refactor across all services, the error/observability gap fixes — log-filter realignment + the
missing per-service / pipeline / DynamoDB alarms + frontend error logging — the cascade-deletion
reconciler sweep + `Rewind/Deletion` metric/alarm, and CloudFront cache invalidation on cascade
delete.)

---

## Next up (after this round)

- [ ] **Enable the deep canary tier in both regions.** Un-suspend `deep` in `helm/canary` (schedules
  already staggered: us-west-2 `06:17` / us-east-2 `18:17`) and add `deep = 86400` to
  `canary_freshness` in both env roots so the freshness alarm covers it.
- [ ] **Fold the region-routing check into the deep canary tier.** The shallow tier already asserts
  in-region routing (the `region-routing` step); add the same check to `deep` once it's enabled.
- [ ] **Assert CDN edge invalidation in the deep canary tier.** The deep canary already deletes its
  seeded video through the real §10.6 cascade and verifies the *origin* (DDB rows + S3 objects) is
  reclaimed, but it does not check that the deleted video's paths were purged at the *edge* (the
  §10.6 `delete-cleanup` CloudFront invalidation). After the cascade-delete step, assert an
  invalidation covering the video's `hls/`, `mp4/`, `thumbnails/` prefixes was issued for the
  distribution — the light version is `cloudfront:ListInvalidations`/`GetInvalidation` (needs that
  read-only grant added to the canary IRSA role), confirming the worker issued it; a stronger version
  would prime an edge-cached object and poll a `cdn.<domain>` GET to 403/404 through invalidation
  propagation (slower, flake-prone — likely the heavier lifecycle canary below). *Prereq: deep canary
  enabled in both regions (above).*
- [ ] **Phase 4 — multi-region resilience drills (NGRH analysis).** Regional fail-away (health-check
  → Route 53 shift), kill a region mid-transcode → confirm the survivor's stuck-`processing`
  reconciler alarms, observe CRR lag/RTC metrics, CloudFront origin-group failover under a regional
  S3 outage. Also validate the in-region resilience added after the node-rotation incident: drain a
  node → confirm PDBs + `/health` target draining keep it graceful (no ALB 5xx) and that
  `alb-elb-5xx` fires when targets are genuinely unhealthy; kill one AZ → confirm the zone
  anti-affinity (2-AZ spread) keeps the region serving without a cross-region failover. *Prereq: deep canary enabled in
  both regions (above).*
- [ ] **CI/CD pipeline.** Automate build → test → push → deploy on git push. Today deploys run from a
  laptop via `deploy.sh` (CI runs on every push, but there's no automated CD).
- [ ] **Seed more fair-use content.** Find 4–5 more free / fair-use / public-domain videos from the
  internet and upload them so the live platform feels more fully featured (better feed, search, and
  surf experience). Verify licensing (CC0 / CC-BY / public domain) and attribute where required.

## Deferred (intentional — with rationale)

- [ ] **Automated, capped re-drive for stuck transcodes.** Build only *if* the stuck-`processing`
  alarm proves noisy. Additive on the existing detect+alarm sweep: SQS-send IRSA +
  `redrive_attempts`/`last_redrive_at` cap + re-enqueue, partitioned to the raw-owner region.
- [ ] **Signed-cookie private ABR ("the right way").** Adaptive HLS for *private* videos via
  CloudFront signed cookies (key group + cookie signing in streaming after the owner check).
  Deferred for the crypto + cross-origin-cookie + CORS-credentials complexity. Today private =
  presigned progressive MP4.
- [ ] **Heavier upload→transcode→play→delete lifecycle canary.** Exercises a real MediaConvert run
  end-to-end; slower + less frequent than the current `deep` tier (which seeds a published video).
- [ ] **Per-service cascade-cleanup fan-out.** Decompose the single `delete-cleanup` worker into
  per-service consumers (EventBridge bus → per-service FIFO queues + scoped IRSA). Only when real
  teams/scale justify it — multi-region does **not** require it.

- [ ] **Map client-caused AWS-SDK errors to 4xx (typed-error follow-up).** Generic AWS SDK failures
  map to `AppError::Internal` (500), which is correct for infra faults (throttle, network,
  permissions) but wrong for the subset that are *client*-caused — most notably `upload /complete`
  with a stale/invalid `upload_id` or mismatched part etags, where S3 returns
  `NoSuchUpload`/`InvalidPart`. Introspect the SDK error code and map those to 400/404. Deferred
  because it needs careful per-operation error-code matching and risks masking real infra 5xx if
  done bluntly; `/complete`'s inputs are server-issued (from `/initiate`), so a mismatch is
  misuse/expiry rather than a normal flow. Low priority until it shows up in practice.

- [ ] **Parameterize the Terraform state backend for forks.** Every root's `backend "s3"` block
  hardcodes `bucket = "rewind-terraform-state"` / `dynamodb_table = "rewind-terraform-locks"` /
  `profile = "rewind"` / `region = "us-west-2"`, and Terraform backend blocks **can't** take
  variables (they're resolved before vars exist). The state bucket name is globally unique in S3, so
  a fork can't reuse it. Fix with partial backend config: strip those to a bare `backend "s3" {}` and
  generate a per-fork (gitignored) `backend.hcl` from an `init.sh`, committing a `backend.hcl.example`
  — the pattern we've used in other projects. Deferred from the open-sourcing tfvars pass (which
  already moved `domain` / `admin_role_arn` / `alert_email` to per-root `terraform.tfvars`) because it
  changes `terraform init` ergonomics across all 7 roots and the `remote_state` data-source reads.
  Intentionally NOT changed by either pass: the `rewind` project-name prefix and the `rewind` AWS
  profile default are kept as-is.

## Future ideas

- [ ] **Content moderation + site admin.** No admin/moderation capability exists today — no way to
  take down an inappropriate video or comment, and no admin role. **Needs a design-iteration pass;
  open questions to resolve before building:**
  - **Admin model:** elevate ("grace") an existing user to admin via a role/flag on `users`, vs. a
    dedicated standalone admin account? How is the first admin bootstrapped?
  - **Authz:** how do services check admin (a claim in the session, a `role` attribute, a separate
    admin-only service/route)? How does it interact with the invite-only gate?
  - **Moderation actions:** takedown of a video (reuse the soft-delete cascade?) vs. comment removal;
    soft hide vs. hard delete; reversibility; audit trail of who actioned what.
  - **Surfacing:** is there an admin UI, or CLI/scripts only (matching the break-glass
    password-reset pattern)? Reporting/flagging flow for users, or admin-initiated only?
  - **Scope for the demo:** likely a minimal admin role + takedown, not a full trust-and-safety
    stack — define the MVP slice during the design pass.

- [ ] **ARC Region Switch (one-click regional failover).** AWS Application Recovery Controller
  routing controls + safety rules, in Terraform. **Needs a design-iteration pass with the team to
  align on what it actually does for us** — latency + health-check routing already auto-fails-away,
  so scope/value (operator-initiated failover, safety rules, the 5-endpoint cluster cost) must be
  defined before building.
- [ ] **Service mesh (Istio / App Mesh).** Adopt when pod-to-pod calls appear (recommendations,
  notifications, DRM/license validation, live chat). Current architecture is hub-and-spoke
  (services → AWS managed services), so a mesh adds complexity without value today.
- [ ] **Canary search-term tuning.** Set a stable `CANARY_SEARCH_TERM` and flip
  `searchExpectHit=true` once a known seed term reliably exists.
- [ ] **Real email flows (verification + forgot-password).** Requires SES production access or
  Amazon Cognito. Only worth it if Rewind ever goes public (it's an invite-only demo; the invite
  code is the real gate, and SES is in sandbox).
- [ ] **Dedicated fault-injection self-test for the stuck-transcode detector.** If ever wanted, build
  it as a separate component emitting under a non-paging metric dimension — *not* bolted into the
  blackbox canary (which can't trigger the CronJob / Scan videos / read CloudWatch).
