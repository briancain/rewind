# Rewind

## Developer Directives (for AI assistants)

When working on this project, follow these rules:

**Build & Quality:**
- Always run `cargo fmt --all` and `cargo clippy --all-targets --all-features -- -D warnings` after Rust changes
- Always run `npm run lint` after frontend changes
- Run tests after changes: `cargo test --all -- --test-threads=1`
- Pipe command outputs to `/tmp` with `tee`, then grep results separately

**Tools & Environment:**
- Use `finch` instead of Docker locally
- Never use `sed` for file editing — use proper file write tools only
- GitHub Actions runs **CI** on every push/PR (cargo fmt/clippy/test, frontend lint/test, terraform validate, helm lint) — but there is **no automated CD**; deploys run from a laptop via `scripts/deploy.sh`
- Everything in Terraform — no manual console configurations

**Workflow:**
- Before starting tasks: assess scope, discuss plan, get alignment, then execute
- **Work in committable units; never commit or push yourself.** At the end of each
  logically-complete unit of work — one task, one functional section, or one cohesive fix that
  builds and passes its checks — STOP and hand the human a ready-to-paste commit message; let them
  commit. Don't run `git commit` / `git push`, and don't start the next unit or deploy until it's
  committed — unless the human says to batch.
- **Commit-message shape:** Conventional Commits subject, imperative, ≤ ~70 chars —
  `type(scope): summary`, type ∈ `feat` / `fix` / `refactor` / `docs` / `chore` / `test` / `perf` /
  `ci`. Scope is optional: either parenthesized (`fix(social):`) or a path-style prefix when it
  reads better (`services/social:`, `infra/observability:`). Body covers *what changed and why*
  (root cause / rationale) and *what was verified* (the checks you ran); note any deferred
  follow-ups. Don't invent ticket numbers.
- **No "god" functions.** Keep functions small and single-responsibility; compose features from
  independently-testable pieces. Follow the codebase's split — thin I/O at the edges
  (`handlers` = HTTP, `repo` = data access) with pure logic pulled into its own functions that
  unit-test without AWS (see `delete-cleanup/decider.rs`, `transcode/reconcile.rs`,
  `transcode/completion.rs`). If one function mixes orchestration, I/O, and business rules, split it.
- Write unit/integration tests for new features and bug fixes
- All new DynamoDB tables must be added to `scripts/local-setup.sh`
- Use `TABLE_PREFIX=test_` in integration tests to isolate from dev data
- When deploying a single service one-off, ensure any env var changes are also reflected in `scripts/deploy.sh`

**Documentation (keep these roles distinct so this doc stays bounded):**
- `docs/DESIGN.md` is the **as-built design** (timeless: architecture + rationale). Keep progress, dates, and status out of it. Sections 1–9 are the original build-with-Kiro design — do not rewrite them.
- Track open / in-progress / deferred work in `docs/TASKS.md`; **delete** a task when it ships (git history is the record of what shipped, and when).
- New work starts as a task in `docs/TASKS.md`, not as a new section here. When it ships, fold the *design* into the as-built section — in the **same commit**, so code, message, and docs agree.

**Accepted trade-offs:**
- Email uniqueness race condition on DynamoDB Global Tables (last-writer-wins)
- No deep health checks (shallow liveness only)
- No service mesh until inter-service HTTP calls are needed

---

## The Pitch

Rewind is an online video streaming platform, similar to YouTube or Netflix, that enables
users to share, upload, and watch videos. Users can upload short or long form video
on their "channel", and once published, other users can watch the video, like or dislike
the video, comment on the video, etc. Movie studios (commercial or independant) can
also upload DRM-protected movies like Netflix that allows users to watch movies.

Users who want to watch videos can interact with the platform in a couple of ways:

- Scroll through a grid or list of videos to select and watch
- Search the platform for videos by key words, or filter by genre, etc
- Use the "Surfing" feature, where they just hit "next" or "back" to go through
a shuffled list of videos and movies from the entire platform, similar to flipping
through TV channels.

Channel owners should be able to upload videos to their account to be published
to the website.

## The technical stuff

This is still to be decided, but, the tech stack should try to use the following
technologies:

- Rust container microservices for the backend
- Typescript for the frontend client
- EKS to run the backend platform microservices. It should be configured with EKS Auto
- Runs entirely on AWS.

This platform has a hard requirement: It _must_ be an Active/Active multi-region
architecture.

We will write our infrastructure as code with Terraform.

## Design Notes with Kiro

### 1. Service Architecture

**Decisions:**

- No custom API gateway service. We use ALB (via AWS Load Balancer Controller) in front
  of EKS with path-based routing. WAF attached for rate limiting and bot protection.
- All application logic lives in EKS as Rust microservices. No Lambdas for core workflows.
- Transcoding uses AWS Elemental MediaConvert for the heavy lifting, but is orchestrated
  by a Rust service in the cluster (not Lambda).
- Social features (likes, dislikes, comments, view counts) grouped into one service for MVP.
  Can split later.
- "Surfing" feature lives as an endpoint in video-catalog for MVP. Can extract later if
  it gains personalization logic.

> ⚠️ **As-built (2026-06-19): WAF is now attached.** Each regional ALB has a WAFv2 web ACL — a
> per-source-IP rate-based rule + an auth-scoped rate-based rule (POSTs to `/login`+`/register`) + the
> AWS managed IP-reputation, common, and known-bad-inputs rule
> groups (default allow). The dedicated paid Bot Control rule set was intentionally omitted. See §10.1.

**Request flow:** Route 53 → CloudFront → ALB → EKS pods
> ⚠️ **Correction (2026-06-17, see §10.1):** the as-built path is **Route 53 → ALB → EKS** direct,
> via per-service subdomains. No CloudFront fronts the ALBs; the only CloudFront is the `cdn.` content
> CDN over the videos S3 bucket.

**Services (7 for MVP):**

| Service | Responsibility |
|---------|---------------|
| **identity** | Auth, user accounts, channel ownership |
| **video-catalog** | Video metadata CRUD, browse/list, surfing endpoint |
| **upload** | Accept video files, store raw in S3, enqueue transcode job via SQS |
| **transcode** | Orchestrator — consumes SQS jobs, copies video to serving bucket, extracts thumbnail + duration via ffmpeg |
| **streaming** | Serve HLS manifests, DRM license proxy, playback session tracking |
| **social** | Likes, dislikes, comments, view counts |
| **search** | Full-text and filtered search over video catalog |

### 2. Multi-Region Active/Active Strategy

> ⚠️ **Superseded in part (2026-06-17): see §10.7.** A full end-to-end review found the implementation
> plan below describes a routing layer (CloudFront → ALB origin group) that was never built and is
> active/passive, and it understates the data-layer and IAM-naming work. **§10.7 is the authoritative
> as-built multi-region design.** The strategic intent below (two regions serving
> equally, Global Tables, S3 CRR, per-region OpenSearch, no cross-region service calls) still holds.

**Decisions:**

- **Regions:** us-west-2 (home) + us-east-2. Both serve traffic equally.
- **Home region (us-west-2):** Terraform state, CI/CD pipelines, Global Table origin.
  Does not affect traffic routing — purely operational convention.
- **Routing:** Route 53 latency-based routing → CloudFront → regional ALB → regional EKS cluster.
- **EKS:** Two independent clusters, one per region. Same Terraform modules, same images,
  deployed independently. No cross-region K8s networking.
- **Consistency model:** Eventually consistent across regions. Acceptable for all MVP use cases.

**Data replication by type:**

| Data Type | Strategy |
|-----------|----------|
| User accounts / auth | DynamoDB Global Tables (last-writer-wins) |
| Video metadata | DynamoDB Global Tables (single-owner write pattern) |
| Video files (S3) | S3 Cross-Region Replication (write-once, no conflicts) |
| Comments / likes / views | DynamoDB Global Tables (append-only / idempotent / atomic counters) |
| Search index | OpenSearch per region, fed from DynamoDB Streams |
| Transcode jobs | Regional only — runs where the upload landed |

**Accepted trade-offs for MVP:**

- Video uploaded in one region may take ~1-2s to appear in the other.
- Password change has a brief window (~<1s) where old credential works in the other region.
- No strong consistency requirements identified for a video platform.
- Email uniqueness can race across regions — two simultaneous registrations with the same
  email in different regions could both succeed (DynamoDB Global Tables conditional writes
  are region-local). Extremely unlikely in practice; mitigations for post-MVP include
  home-region routing for registration or post-hoc reconciliation.

**Multi-region implementation plan (execute in order):**

Step 1: CloudFront + ACM (global module)
- Create ACM certificate in **us-east-1** (required by CloudFront) for `*.watch.example.com`
- Create CloudFront distribution with origin group (us-west-2 ALB + us-east-2 ALB)
- Origin failover: if primary returns 404/5xx, CloudFront tries secondary origin
- Update Route 53 to point at CloudFront distribution (not directly at ALBs)
- CloudFront handles all TLS termination; ALBs become HTTP-only internal

Step 2: DynamoDB Global Tables
- Enable Global Tables on all 11 DynamoDB tables (adds us-east-2 replica)
- Zero downtime — existing us-west-2 data automatically replicates
- No application code changes needed

Step 3: S3 Cross-Region Replication
- Enable bi-directional CRR on `videos` and `thumbnails` buckets
- Raw bucket stays regional (transcode runs where upload lands)
- Video availability during replication delay: CloudFront origin failover handles transparently

Step 4: Deploy us-east-2 environment
- Create `infra/environments/dev/us-east-2/` (same modules, different region variable)
- Creates: VPC, EKS, ECR, SQS queue, OpenSearch domain, IRSA roles
- Regional ACM cert for the ALB (separate from CloudFront cert)
- Deploy all services via `deploy.sh` pointed at us-east-2

Step 5: Search indexing pipeline (both regions) — **search consumer IMPLEMENTED 2026-06-11 (Section 13)**
- [x] Build DynamoDB Streams consumer in search service (replaces `SEARCH_ENDPOINT` from transcode)
- [x] Remove `SEARCH_ENDPOINT` env var from transcode cloud deploy (no longer needed; local shim kept)
- Each region's search service subscribes to its local DynamoDB Streams replica (via per-region Pipe + queue)
- Both OpenSearch domains converge to same indexed state independently
- Per region bring-up: run the `scripts/reindex.sh` Job to seed that region's OpenSearch

Step 6: Validate
- Upload video in us-west-2 → verify playable in us-east-2 (via CloudFront failover)
- Upload video in us-east-2 → verify playable in us-west-2
- Search works in both regions independently
- Simulate region failure → traffic shifts to surviving region

**Key design principle:** No cross-region service calls. No custom fallback logic in
application code. All multi-region concerns handled at the infrastructure layer
(CloudFront, Global Tables, CRR, DDB Streams).

**No application code changes required** except:
- Search service: add DynamoDB Streams consumer (new feature)
- Streaming service: return CloudFront URLs instead of direct S3 presigned URLs
- Remove `SEARCH_ENDPOINT` from transcode once Streams consumer is live

### 3. Storage, CDN, and Video Pipeline

**Video lifecycle:**

```
User upload → upload service → raw S3 bucket → SQS → transcode service → ffmpeg (thumbnail + duration)
→ video copied to processed S3 bucket → CloudFront → viewer
```

**Future enhancement:** Replace ffmpeg copy with AWS MediaConvert for HLS adaptive bitrate
ladder (360p/720p/1080p/4K segments) and DRM support when volume or viewer experience demands it.

**S3 buckets (per region):**

| Bucket | Purpose | Lifecycle |
|--------|---------|-----------|
| `rewind-raw-{region}` | Raw uploaded files | Delete 30 days after successful transcode |
| `rewind-video-{region}` | HLS segments + manifests | Permanent, served via CloudFront. CRR to other region. |
| `rewind-thumbnails-{region}` | Generated + custom thumbnails | Permanent, served via CloudFront. CRR to other region. |

- S3 CRR on processed and thumbnail buckets only. Raw files stay regional.
- **Known gap:** Raw bucket lifecycle deletes after 30 days from upload, not from successful
  transcode. If a job fails and is never retried, the source file is lost after 30 days.
  Future improvement: delete raw files explicitly after successful transcode, or tag objects
  with transcode status and scope the lifecycle rule to only delete tagged/completed files.

**Upload:**

- 5GB max file size.
- S3 multipart upload via presigned URLs — large files go directly to S3, not through pods.
- Upload service initiates multipart, returns presigned URLs to client, then completes upload.

**Transcoding (MediaConvert):**

- Output format: HLS (widest device compatibility).
- Bitrate ladder: 360p, 720p, 1080p (4K post-MVP).
- Segment duration: 6 seconds.
- Thumbnail auto-generated from frame at ~25% of video duration.

**Thumbnails:**

- Default: auto-generated by MediaConvert during transcode.
- Users can upload a custom thumbnail to override (stored in thumbnails bucket, metadata updated in catalog).

**CloudFront:**

- Single distribution, two S3 origins (one per region) in an origin failover group.
- Long TTL for video segments (immutable). Short TTL for manifests.
- Signed URLs for access control (logged-in users only).

**DRM:** Deferred to post-MVP. Signed CloudFront URLs provide access control for now.

### 4. Data Layer

**Decisions:**

- DynamoDB Global Tables for all persistent data.
- Table-per-service (not single-table design) — clearer ownership, easier capacity management.
- Users and channels are the same entity. Every user has a channel.
- Flat comments for MVP. Threading can be added later via `parent_comment_id` + GSI.
- Surfing uses deterministic shuffle over video IDs (seed + offset) — no extra table.

**Tables:**

| Table | Service | PK | SK | GSIs |
|-------|---------|----|----|------|
| `users` | identity | `user_id` | — | `email` (login lookup) |
| `sessions` | identity | `session_token` | — | — (TTL for expiry) |
| `videos` | video-catalog | `video_id` | — | `channel_id+created_at`, `status+created_at`, `genre+created_at` |
| `comments` | social | `video_id` | `created_at#comment_id` | — |
| `reactions` | social | `video_id#user_id` | — | — |
| `view_counts` | social | `video_id` | — | — (atomic counter) |
| `transcode_jobs` | transcode | `job_id` | — | `video_id` |

> ⚠️ **As-built correction (see §10):** there are **11** tables, not 7. `view_counts` is actually
> **`video_stats`**, and `reactions` is **PK `video_id` + SK `user_id`** (not a single composite hash).
> The full set: `users`, `sessions`, `verification_tokens`, `videos`, `comments`, `reactions`,
> `video_stats`, `comment_reactions` (PK `video_id`, SK `{comment_id}#{user_id}`), `view_history`
> (PK `user_id`, GSI `video-id-index`), `invite_codes`, `transcode_jobs`. See §10.6 for the
> cascade-delete schema choices.

**MVP access patterns:**

- Get video by ID (PK lookup)
- List videos by channel (GSI, sorted by created_at)
- List latest published videos — global feed (GSI on status+created_at)
- Get user by ID / by email
- List comments for a video (sorted by time, paginated)
- Get like/dislike status for user+video
- Get view/like/dislike counts
- Search — handled by OpenSearch, not DynamoDB
- Surfing — deterministic shuffle, no extra storage

**Deferred to post-MVP:** Trending, watch history, subscriptions/follows, recommendations.

### 5. Search and Discovery

**Decisions:**

- OpenSearch (managed) per region. No cross-region replication — each region indexes
  independently from DynamoDB Streams.
- Indexing pipeline lives in EKS: search service consumes DynamoDB Streams (via Kinesis
  Data Streams adapter) and writes to OpenSearch.
- Ranking: BM25 text relevance + boost by view count.
- Autocomplete/typeahead via OpenSearch completion suggesters (stretch goal for MVP).

> ⚠️ **As-built correction (see §10.4):** indexing is **EventBridge Pipes → SQS FIFO → consumer**
> off the `videos` DynamoDB stream — **not** a Kinesis Data Streams adapter. Ranking is **plain BM25**;
> the view-count boost was not implemented. Autocomplete was not built.

**Indexed fields:** title, description, tags, genre, channel name, upload date, view count.

**MVP query capabilities:**

- Full-text search across title + description + tags
- Filter by genre, filter by channel
- Sort by relevance or recency
- Autocomplete on title + tags (stretch goal)

**Data flow:**

```
DynamoDB videos table → DynamoDB Streams → Kinesis adapter → search service (EKS) → OpenSearch
```

Each region's search service indexes from its local DynamoDB Streams replica, so both
OpenSearch domains converge to the same data.

### 6. Project Structure and Repo Layout

**Decisions:**

- Monorepo.
- Rust workspace for all backend services — shared `Cargo.lock`, common `shared` crate.
- Frontend: Next.js (App Router) with TypeScript. SSR for SEO on video/channel pages,
  image optimization for thumbnails, HLS.js for client-side video playback.
- Terraform with per-region directories under each environment. Global resources separate.
- Helm for Kubernetes manifests — one chart template, per-service values files.

**Layout:**

```
rewind-service/
├── docs/
│   └── DESIGN.md
├── services/                    # Rust workspace
│   ├── Cargo.toml               # Workspace root
│   ├── shared/                  # Shared crate (DDB client, auth middleware, errors, tracing)
│   ├── identity/
│   ├── video-catalog/
│   ├── upload/
│   ├── transcode/
│   ├── streaming/
│   ├── social/
│   └── search/
├── frontend/                    # Next.js + TypeScript
│   ├── package.json
│   └── src/
├── infra/                       # Terraform
│   ├── modules/                 # Reusable (eks-cluster, dynamodb-table, s3-bucket, etc.)
│   ├── environments/
│   │   ├── dev/
│   │   ├── staging/
│   │   └── prod/
│   │       ├── us-west-2/
│   │       └── us-east-2/
│   └── global/                  # Route 53, CloudFront, IAM
├── helm/                        # Helm chart
│   ├── rewind-service/          # Shared chart template
│   └── values/                  # Per-service values (identity.yaml, upload.yaml, etc.)
├── docker/                      # Dockerfiles per service
└── scripts/                     # Dev tooling, CI helpers
```

> ⚠️ **As-built correction:** only **`infra/environments/dev/{us-west-2,us-east-2}`** exists — there
> is **no `staging`/`prod`** (this is a single-account demo). *A real service would carry
> `dev`/`staging`/`prod` stages.* Also as-built: the service list includes **`delete-cleanup`** and
> **`canary`** crates; `infra/` also has `bootstrap/`, `cdn/` (where CloudFront lives, not `global/`),
> `data/` (the region-neutral Global Tables), and `observability/`; `helm/` also has a `canary/`
> chart; and the frontend uses `app/` (App Router), not `src/`.

### 7. Local Development

**Note:** We use `finch` locally instead of Docker. Commands are interchangeable
(`finch compose up` instead of `docker compose up`, etc.).

**Approach:** Docker Compose for local dependencies, Rust services run natively.
Single command startup via `./scripts/dev.sh`.

```bash
./scripts/dev.sh        # Start everything, see combined color-coded logs
                        # Ctrl+C stops services, containers stay running for fast restart
./scripts/local-stop.sh # Full teardown including containers
```

`dev.sh` handles:
1. Starts Finch containers (DynamoDB Local, LocalStack, OpenSearch)
2. Waits for readiness (health polling, not sleep)
3. Creates DynamoDB tables, S3 buckets, SQS queues (idempotent)
4. Builds the Rust workspace
5. Starts all 7 backend services + Next.js frontend
6. Traps Ctrl+C for clean shutdown

**Local mode behavior (gated by env vars, no prod impact):**

- `DISABLE_MEDIACONVERT` — transcode service copies raw file to videos bucket,
  extracts thumbnail via ffmpeg, sets status to `published`. Video plays as native mp4.
- `SEARCH_ENDPOINT` — transcode service indexes video in OpenSearch after publishing
  (prod uses DDB Streams instead).
- `S3_ENDPOINT` + `force_path_style` — streaming service generates path-style presigned
  URLs for LocalStack compatibility.

**Data:** DynamoDB Local is ephemeral — data is lost on container teardown.

**Test isolation:** Integration tests set `TABLE_PREFIX=test_` so they create/delete
`test_videos`, `test_sessions`, etc. without touching dev data.

### 8. Testing

**Requirement:** All services must include unit and integration tests.

- **Unit tests:** Per-service, run with `cargo test`. Mock external dependencies (DDB, S3, SQS).
  Test business logic, data transformations, error handling.
- **Integration tests:** Run against Docker Compose stack (DynamoDB Local, OpenSearch, LocalStack).
  Test real API calls, database operations, queue interactions.
- **Frontend:** Jest/Vitest for unit tests, Playwright or Cypress for E2E.
- **CI:** All tests run on every PR. Integration tests use Docker Compose in CI.

### 9. Domain and DNS

**Domain:** `watch.example.com`

- The parent domain `example.com` is managed in a separate AWS account.
- Rewind infrastructure lives in its own AWS account.
- Cross-account delegation: the parent account adds an NS record for `watch.example.com`
  pointing to the Rewind account's Route 53 hosted zone nameservers.
- All routing (latency-based, CloudFront, ALB) managed in the Rewind account.

### 10. As-Built System (post-initial-design)

> Sections 1–9 are the **initial design** (the build-with-Kiro record), preserved as written. The
> platform has since been built out and extended; this section is the **as-built** design — what the
> system actually is today where it differs from or adds to the above. Where §1–9 and this section
> disagree, this section wins. Open/remaining work lives in `docs/TASKS.md`; the dated build history
> is in `git log`.

#### 10.1 Request flow & routing (as-built)

Browser → **Route 53 (latency record + per-region health check) → regional ALB (host-based Ingress) → EKS pod**. There is **no CloudFront in front of the services** (the §2 "CloudFront → ALB origin group" routing layer was never built; it would have been active/passive). Each backend is a public subdomain (`identity.`, `catalog.`, `upload.`, `streaming.`, `social.`, `search.`), plus the apex `watch.` (frontend) and a `*.` wildcard — all latency-routed A-aliases to the region's ALB with `set_identifier = <region>`. A region's health check failing auto-removes it from the latency set (automatic fail-away; **AWS ARC not used**). A region-pinned host `<region>.watch.<domain>` aliases directly to that region's ALB (drives the health check, and the canary's region-routing assertion). The only CloudFront is the content CDN at `cdn.<domain>` over the videos bucket (§10.3). Each region's ALB sits behind a **regional WAFv2 web ACL** (default allow) carrying **two rate-based rules plus** the AWS managed IP-reputation, common, and known-bad-inputs rule groups. The rate limiting is deliberately two-tier: a **global** per-source-IP limit (2000 req / 5 min) sized for volumetric abuse, and a much tighter **auth-scoped** limit (100 / 5 min per IP) whose scope-down statement matches only **POSTs** to `/login` + `/register`. The global limit alone is roughly an order of magnitude above what one IP needs to run an effective credential / invite-code guessing loop, and lowering *it* would rate-limit ordinary read traffic arriving from behind a shared NAT or corporate egress — hence a separate, narrower rule rather than a lower global number. The POST requirement is load-bearing: the frontend serves `GET /login` as a page, so matching the path alone would count ordinary page views toward the budget. The paid Bot Control rule set is intentionally omitted. Every service exposes `/health` (200) and the ALB target groups health-check that path, so target health is real and a node drain deregisters targets gracefully. Each Deployment runs with a PodDisruptionBudget (`maxUnavailable: 1`) and **required zone pod anti-affinity** (two replicas of a service never share an AZ), so replicas deterministically span two AZs and a Karpenter node rotation or a single-AZ loss stays graceful rather than dropping the region. The NodePool caps voluntary disruption to one node at a time.

#### 10.2 Video visibility

`videos.visibility` ∈ {`public` (default), `unlisted`, `private`}, independent of `status` (the transcode lifecycle). public = feed + search + direct link; unlisted = direct link only (hidden from feed/search); private = owner-only. Enforced in **catalog** (feed/list filter `visibility = public`), **streaming** (private requires `caller == channel_id` before issuing a URL), and **search** (indexes only public). A soft-deleted video (`status = deleted`) is treated as not-found everywhere.

#### 10.3 Transcode & delivery (MediaConvert + CloudFront)

One MediaConvert job per upload emits three output groups: **HLS (Automated ABR)** → `videos/hls/{id}/`, a **progressive MP4** → `videos/mp4/{id}/`, and a **frame-capture thumbnail** (~25% via the MediaConvert `Probe` API) → `videos/thumbnails/{id}/` (thumbnails live under the **videos** bucket and are served by streaming as short-lived presigned URLs — there is **no dedicated thumbnails bucket**; the §3 design's separate bucket was never wired up and has been removed). Completion is event-driven, **no Lambda**: MediaConvert → EventBridge rule (`detail.status ∈ {COMPLETE, ERROR}`) → SQS completions queue → a second transcode consumer that reads the authoritative master `.m3u8`, MP4, thumbnail, and duration from the event and publishes (`status = published`, `manifest_url = cdn…`), or marks `failed`. Delivery: **public/unlisted → HLS via CloudFront `cdn.<domain>`** (OAC, us-east-1 viewer cert, CORS for hls.js); **private → short-lived presigned MP4**, issued only after streaming's owner check. Local dev keeps the ffmpeg fallback (`DISABLE_MEDIACONVERT=1`). DLQs on the transcode-jobs queue, the completions queue, and the EventBridge completions target. Frontend playback is a Media Chrome `<hls-video>` player with an ABR quality menu (`frontend/components/VideoPlayer.tsx`), loaded via `next/dynamic({ ssr: false })`. *Deferred: signed-cookie ABR for private video — see TASKS.md.*

#### 10.4 Search index sync

OpenSearch is kept in sync per region off the `videos` DynamoDB stream, with no synchronous cross-service calls: stream (`NEW_AND_OLD_IMAGES`) → **EventBridge Pipe** (filtered to the videos table) → **SQS FIFO** (`MessageGroupId = video_id`, + DLQ) → search consumer → OpenSearch. Indexing rule (`search/src/indexer.rs`): upsert iff `status = published && visibility = public`, else delete — idempotent by `video_id`, so at-least-once delivery is safe. Each region consumes its **local** replica stream, so both domains converge independently. Seeding/rebuild is an in-cluster Job (`scripts/reindex.sh` → `service reindex`) under the search IRSA role — not a public route. (This replaced the old transcode `SEARCH_ENDPOINT` HTTP shim, which is kept only for local dev. Pipes were chosen over a hand-rolled Streams consumer to offload shard coordination/checkpointing, and over Lambda per the no-Lambda directive.) The service talks to OpenSearch through the **official `opensearch-rs` client**, SigV4-signed via its `aws-auth` feature using the search IRSA role (one `videos` index); the query path and the consumer's index/delete writes share it. The client carries a bounded request timeout + a single transport-error retry (env-tunable) so a stalled OpenSearch call fails fast and recovers rather than hanging.

#### 10.5 Account & auth additions

`POST /change-password` (authenticated; verifies the current password, enforces length, and invalidates the user's *other* sessions via a `user_id` GSI on `sessions`); `/logout` deletes the server session row; a `hash-password` CLI + `scripts/admin-reset-password.sh` provide break-glass reset. Email verification + forgot-password were intentionally dropped (SES sandbox + invite-only demo — the invite code is the real gate). The sessions table TTL is keyed on the `ttl` attribute.

#### 10.6 Cascade deletion of a video's dependent data

`DELETE /videos/{id}` **soft-deletes** (`status = deleted`, `deleted_at`, numeric `purge_at`) instead of hard-deleting: a soft-delete is a MODIFY that replicates safely under Global Tables, whereas a hard `DeleteItem` can be **resurrected** by a concurrent cross-region write (last-writer-wins). The publish path is **conditional** (`status <> deleted`) so a late MediaConvert completion can't un-delete a video. The soft-delete flows `videos` stream → a second **filtered** EventBridge Pipe → SQS FIFO → the **`delete-cleanup` worker**, which reclaims all dependent data: social rows (`comments`, `reactions`, `comment_reactions`, `video_stats`, `view_history`), S3 objects (`hls/`, `mp4/`, `thumbnails/`, `raw/`), and the `transcode_jobs` record. Search self-cleans (a soft-deleted video fails the index-eligibility rule). A TTL finalizer on `purge_at` hard-deletes the tombstone after a grace period. The worker runs **per-region** (CRR does not replicate deletes). Schema enabling a single `Query(video_id) → batch delete` per table: `comment_reactions` re-keyed to PK `video_id` / SK `{comment_id}#{user_id}`; `view_history` gained a `video-id-index` GSI. A per-region **`reconcile` CronJob** (the delete-cleanup image run as `delete-cleanup reconcile`) closes the cleanup-side observability gap: it scans the `videos` Global Table for `deleted` tombstones older than a grace threshold (`DELETION_RECONCILE_THRESHOLD_MINS`, default 30 min — beyond the cleanup queue's redrive window so in-flight cleanup is never raced) and **probes** each — read-only — for any dependent row or S3 object the worker should have reclaimed, emitting `Rewind/Deletion UnreclaimedDeletions` (+ an alarm). This catches the two silent failures no DLQ sees: a `videos-to-cleanup` Pipe that never enqueued the event, and a partial cleanup that deleted its message on an incomplete "success". Detection is necessarily two-stage — the worker never writes the `videos` row (a `deleted → deleted` MODIFY would loop back through the Pipe filter), so a tombstone alone can't reveal whether cleanup ran: a pure candidate filter (`status = deleted` + age), then the dependent-store probe. **Detect + alarm only** — automated re-cleanup is deferred (mirrors the transcode reconciler's deferred re-drive), so the reconcile IRSA role is read-only. After deleting the origin S3 objects the worker also **invalidates** the video's `hls/`, `mp4/`, and `thumbnails/` prefixes on the content CDN (`cdn.<domain>`), so a deleted video is gone at the edge too — not just at origin (HLS segments are immutable + long-TTL, so without this they could keep playing from edge cache for hours). Invalidation runs only where a distribution id is configured (`CDN_DISTRIBUTION_ID`, absent locally / before the `cdn` stack exists) and is ordered after the S3 deletes (invalidating earlier would let a request in the gap re-cache the live object); a failure redrives the (idempotent) cleanup message, and a persistent failure dead-letters into the existing cleanup DLQ alarm, so "gone at the edge" is guaranteed rather than best-effort. The grant is `cloudfront:CreateInvalidation` on `*` (the distribution id is random and the env can't read the `cdn` stack without a dependency cycle). *Deferred: per-service fan-out — see TASKS.md.*

#### 10.7 Multi-region active/active (as-built — supersedes the §2 plan)

Live across **us-west-2 + us-east-2**, both serving traffic. Resource naming: DynamoDB table names are region-free (a Global Table shares one name); regional resources live in per-region namespaces; **IAM roles are region-qualified** (account-global, so they can't collide across regions).

- **Data:** all 11 DynamoDB tables are **Global Tables** (owned once in the region-neutral `infra/data` stack; replicas added per region). Streams are enabled on all of them for the Pipes above.
- **Object storage:** **bidirectional S3 CRR with Replication Time Control (15-min SLA)** on the `videos` bucket — which holds the HLS, MP4, *and* thumbnail outputs, since there is no separate thumbnails bucket (§10.3); the raw bucket stays regional (auto-expires 30 days). Seeding a new region is a one-time **S3 Batch Replication** job (`scripts/backfill-replication.sh`). The CRR **peer is derived in code** from a region→peer topology map; replication is gated by an explicit `enable_replication` flag defaulting to `true`, so a bare `terraform apply` never silently disables CRR.
- **Content delivery:** a **single global CloudFront distribution with an origin group** (primary = us-west-2 videos bucket, failover = us-east-2) + Origin Shield. `manifest_url` stays one global `cdn.<domain>`; per-object 404-failover serves content present in only one region, so publish is immediate (no replication-gated publish). MRAP was rejected as the origin (it mandates Lambda@Edge for SigV4A and doesn't solve per-object freshness).
- **Routing / failover:** Route 53 latency records + per-region health checks → automatic fail-away (§10.1). ARC remains a documented future option, not built.
- **Per-region transcode + the region-failure gap:** an upload is transcoded where it landed. A region dying mid-transcode (or a lost completion event) strands a video in `processing` with no *failed* message to redrive. A per-region **`reconcile` CronJob** (the transcode image) scans the `videos` **Global Table** — so a *surviving* region can see a *dead* region's stranded rows — and emits `Rewind/Transcode StuckTranscodes` + an alarm. Recovery is currently a manual redrive (`scripts/redrive-transcode.sh`); automated capped re-drive is deferred (TASKS.md). The EventBridge completions target also has a delivery DLQ.
- **Per-region OpenSearch (single-node, VPC-attached — deferred Multi-AZ):** each region runs a **single-node `t3.medium.search`** domain (no Multi-AZ, no dedicated master), **attached to the VPC** — its ENI lives in one private subnet behind a security group that admits HTTPS only from within the VPC, so the search pods reach it on the AWS private network (no NAT/public-internet hop). It is reached via the official `opensearch-rs` client, SigV4-signed with the search IRSA role. The index is rebuildable per region from the `videos` Global Table (`scripts/reindex.sh`), so a lost domain is recoverable, but today a node/AZ loss takes that region's search read/sync down until the node is replaced. (The node was bumped from `t3.small` to `t3.medium`: the small's ~1 GB heap ran chronically at 65–74% JVM memory pressure — grazing the GC threshold, so the single node periodically stalled for tens of seconds and hung `/search` until the search client's timeout → 500 → shallow-canary flap; `t3.medium` doubles the heap to ~35% steady pressure and drops the burstable-CPU variability. It stays single-node / single-AZ by design — the redundancy gap is intentionally retained for the resilience analysis.) (The domain was originally a public endpoint; that path's *new-connection* TLS handshakes intermittently stalled 12–24s — invisible in OpenSearch's own metrics since query execution stayed <100ms — and, because the hourly canary was the only search caller, every run built a cold connection and periodically blew the search client's 12s×2 timeout → 500 → shallow-canary alarm. The VPC ENI path removed that.) **Deferred:** move to a multi-AZ, right-sized domain — sequenced behind the Phase-4 resilience drills (TASKS.md), which are expected to surface single-AZ search as the concrete trigger.
- **No cross-region service calls;** all region-awareness is infra/env-driven (region from the SDK default; buckets via env; tables via `TABLE_PREFIX`).

#### 10.8 Observability & canary

A **global** multi-region CloudWatch dashboard (owned once in `infra/observability` — dashboards are account-global) + **per-region** alarms routed by severity to two SNS email topics (`infra/modules/observability`) — an outage-class **page** topic (region/edge down via ALB 503s, OpenSearch cluster RED, and the end-to-end canary journeys) and a lower-severity **ticket** topic for everything else, so a routine scanner-driven WAF spike no longer pages at the same urgency as an outage (both default to the same recipient until a pager is wired up); container logs ship via the CloudWatch Observability addon.

**Request attribution.** Alarms tell you *that* traffic is abnormal; attributing it to a source is a separate capability, and it is deliberately provided twice because each half has a blind spot. **(1) Every regional WAF web ACL logs every evaluated request** to a `aws-waf-logs-<name>-alb` CloudWatch log group (14-day retention) — client IP, method, URI, headers, country, matched rules. All requests are logged, not just blocked ones: under a default-allow ACL the traffic worth attributing is precisely what was *allowed*. `wafv2:GetSampledRequests` is not a substitute — it returns a sample and retains only 3 hours, so an investigation opened the next morning has nothing to work from. The `authorization` and `cookie` headers are **redacted** so a session bearer token can't be replayed out of the log group. WAF's `logging_filter` can only match on action or rule label, so health-check noise (the bulk of steady-state volume) can't be filtered out; retention is kept short to bound cost instead of sampling and losing the signal. **(2) Every service's own request log carries the client IP** — `shared::middleware` puts it on the request span (so it is attached to the response line *and* to any error logged inside the request), which is what joins an application 4xx/5xx to a source address for the full log-group retention. It is read as the **last** `X-Forwarded-For` entry, because the ALB *appends* the peer address it observed (`xff_header_processing.mode = append`) and every earlier entry is caller-supplied — taking the leftmost would let an attacker forge its own source. Nothing fronts the ALBs (§10.1), so the ALB-observed peer is the real client. ALB access logs are intentionally not enabled: they would mostly duplicate these two, and turning them on means changing the Ingress annotations that provision the live ALB (tracked in `docs/TASKS.md`).

Log-based metric filters key on the application JSON as parsed by the CloudWatch Observability addon (Fluent Bit), which nests each service's structured log line under `$.log_processed` — so response status is matched at `$.log_processed.fields.status`, request path at `$.log_processed.span.path`, and log messages at `$.log_processed.fields.message`; the frontend emits its own flat structured error line (a Next.js `instrumentation.ts` `onRequestError` hook) whose top-level `status` lands at `$.log_processed.status`, so it is observable alongside the Rust services. Alarms cover infra + customer-experience: **ALB 5xx** (both ALB-generated `HTTPCode_ELB_5XX` — the no-healthy-host/503 signal — and target 5xx) **+ p95 latency** (the `LoadBalancer` dimension is passed in dynamically from the env root, never hardcoded); a **per-service 5xx alarm for every request-serving service** (identity, video-catalog, upload, streaming, social, search) **and the frontend**, plus an **upload customer-experience alarm** on any 4xx/5xx on `/uploads/*` (distinct from upload's 5xx alarm — it also catches the client-caused 4xx that `from_aws` now returns for a stale/unassemblable multipart upload, which the 5xx alarm no longer sees); the search-index DLQ/Pipe and the **cascade-cleanup DLQ + `videos-to-cleanup` Pipe + the unreclaimed-deletion reconciler** (`Rewind/Deletion`, the cleanup-side analogue of the stuck-transcode reconciler); the **transcode-jobs DLQ** and the stuck-transcode reconciler; **S3 CRR and DynamoDB Global Table** replication-latency + throttle (both gated on a peer region); a **WAF blocking** trio — a *global rate-limit-rule* alarm (the real-user-impact signal: legitimate clients rate-limited at the edge, e.g. behind a shared NAT), an *auth-rate-limit-rule* alarm (a single source throttled while guessing credentials / invite codes — the control working, so it records an attempt rather than reporting breakage), and a high, sustained *aggregate* blocked-requests alarm (a blocking flood, tuned well above the routine scanner-blocking baseline so ordinary internet background-radiation scans don't fire it); an **L7-attack detection set** designed to fire on *harm and signature*, not raw request volume — volume alone can't separate an attack from a flash crowd / launch / viral video (all spike `RequestCount` identically), and on a near-idle baseline (~150 req/5min, mostly Route 53 health checks) even a couple of chatty real browser sessions cross a trained band. So: the raw `RequestCount` anomaly alarm is a **silent input to a composite** (`request-flood-harmful`) that fires only when the volume anomaly coincides with actual harm — target/ELB 5xx, high p95 latency, or edge rate-limiting — i.e. a flood degrading the platform, not a busy day; two **baseline-independent error-*ratio* alarms** (5xx and 4xx as a percent of `RequestCount`, guarded by a `RequestCount` volume floor so a few errors against idle traffic can't read as 100%) that mean the same thing at 150 or 1.5M req/5min; and an **app-wide 404 vuln-scan alarm** (a path-probing sweep for nonexistent paths produces a 404 burst that the default-allow WAF managed rules don't block, since they match known exploit signatures rather than mere nonexistence). The raw volume anomaly is retained as a **dashboard diagnostic** (an L7-volume-vs-band widget plus an edge-errors/WAF-blocks widget) — the "why" you consult when the composite fires; **OpenSearch JVM-memory-pressure + CPU** saturation alarms (the documented single-node read-path degradation that precedes cluster-RED — see §10.7); **auth (`/login`+`/register`) and feed 4xx spikes** (the 4xx blind spot the per-service 5xx alarms miss), **per-service p95 request latency** (from the middleware's `latency_ms`, catching a single slow journey the aggregate ALB p95 averages out), and a **client-side playback-error** signal (VideoPlayer beacons hls.js/MediaError failures to `/api/playback-error`, catching "won't play in the browser" when the server returned 200s); and the canary. Two alarm types are **global** (their metrics are published only in us-east-1) so they live in the `infra/cdn` stack on a dedicated us-east-1 SNS topic: **CloudFront 5xx + total error rate** (the content-delivery path viewers actually hit, invisible to the ALB/per-service alarms) and **Route 53 health-check status** per region (a region silently failing away).

**Alerting stance (symptom-based).** Alarms are scoped to *customer-facing breakage* and *silent correctness breakage* — an alert should mean something is actually broken for a user (or a data path is silently diverging and will be), not merely that a resource is warm. Saturation and leading signals — node/pod CPU + memory, per-service running-pod counts, pod restarts, ALB unhealthy-host count, DynamoDB `SystemErrors`, and transcode completions-queue age — are therefore **dashboard diagnostics, not alarms** (a per-region "diagnostics" widget row), since on their own they don't imply customer impact: a crash-looping pod with a healthy peer, a node near its CPU limit, or lost redundancy is not an outage. They are the context you consult *once a symptom alarm fires*. The OpenSearch JVM-memory-pressure/CPU alarms are the one retained leading-indicator exception — kept as low-severity early-warnings because they precede a *documented* hard-down of the single-node domain (§10.7). Two symptom/silent-breakage additions round out the set: a **log-pipeline heartbeat** — alarms when `AWS/Logs IncomingLogEvents` on the application log group drops to zero, with `treat_missing_data = breaching`, because ~half the customer-experience alarms are log-metric-filters that fail *silent* (a broken Fluent Bit / addon or a drifted log-JSON shape makes them stop firing), so the absence of logs must itself alarm; and a **transcode-completions SQS DLQ** alarm — the consumer-side counterpart to the existing EventBridge-delivery DLQ alarm, catching a completion event the publish consumer repeatedly fails to apply (which strands a video in `processing`). Outage-class alarms (region/edge down via ALB 503s, OpenSearch cluster RED, the end-to-end canary journeys, and the log-pipeline heartbeat) route to the **page** SNS topic; everything else to the **ticket** topic.

The **canary** (`services/canary`) is a blackbox binary that hits the public `*.<domain>` endpoints, run in-cluster as CronJobs (dedicated `helm/canary` chart, scoped IRSA role):
- **`shallow`** (hourly, read-only): health of every service + public feed + a search query + an **error-contract probe** (a blank JSON body field and a blank query param, both headed for a DynamoDB key, must return 4xx — never 5xx; the blackbox guard on §10.10's classification, which unit tests can't provide since they can't catch a handler that stopped calling its validator) + a **region-routing DNS check** — it resolves a latency-routed host and this region's region-pinned host and asserts they hit the same ALB, proving Route 53 kept this region's traffic in-region.
- **`deep`** (full multi-actor journey): seeds an unlisted video, exercises auth/social/streaming, then deletes it via the **real §10.6 cascade and verifies every dependent resource is reclaimed** — so it doubles as the cascade's continuous validator.

It emits `Rewind/Canary` metrics (per-step + overall `CanarySuccess`); alarms cover overall failure, region-routing failure, and freshness (a *stopped* canary). Schedules are staggered per region; **`deep` is suspended** pending vetting (TASKS.md). On-demand runs via `scripts/canary.sh`.

#### 10.9 Accepted trade-offs (active/active)

Eventually consistent across regions; email-uniqueness can race (LWW — conditional writes are region-local); a login may not be visible in the other region for ~1–2s (a request routed there in that window can get a spurious 401); metadata edits are last-writer-wins. All acceptable for a video platform. (Plus the §3 raw-bucket lifecycle gap: raw is deleted 30 days from upload, not from successful transcode.)

#### 10.10 Cross-cutting decisions

**No Lambdas for core workflows** — all event-driven work is **EventBridge/Pipes → SQS → EKS Rust consumers** (search sync, cascade cleanup, transcode completion). Table-per-service ownership. Soft-delete for resurrection-safety under Global Tables. No service mesh until pod-to-pod calls actually exist (current shape is hub-and-spoke: services → AWS managed services). **One shared error type** — `shared::error::AppError` (a `thiserror` enum) is the platform error vocabulary: request-serving handlers return it (it is `IntoResponse`, mapping `NotFound`/`BadRequest`/`Unauthorized`/`Forbidden`/`Conflict` → the matching 4xx and only `Internal` → 500), and the SQS-worker services use the same type. Expected/business failures map to 4xx, so a 5xx genuinely means an *unexpected* fault — which keeps the per-service 5xx alarms meaningful. Client-caused AWS-SDK errors are classified by service code via `AppError::from_aws` (which also logs the underlying code — an `SdkError`'s bare `Display` is only the opaque `"service error"`): e.g. upload `/complete` on a stale/invalid `upload_id` or mismatched parts returns S3 `NoSuchUpload`/`InvalidPart`, mapped to 4xx, so only genuine infra faults (throttle, network, permissions, timeouts) remain 500. The DynamoDB side has the same shape in two layers, because DynamoDB rejects an *empty string in a key attribute* with a `ValidationException` — so any caller-supplied value that lands in a table key or a GSI key condition (`{"email": ""}`, `?channel_id=`, `?watched_at=`) would otherwise turn a client mistake into a 500 that any unauthenticated caller could use to drive the per-service 5xx alarms at will. **(1) Guard at the edge:** `shared::validate` (`non_empty` / `max_len` / `key_field`) is applied in the handler before the repo call, so the 400 names the offending field; identity's `register`/`login` rules live in a pure, unit-tested `identity::validate` (which also owns the single password-length rule shared with `/change-password`). **(2) Classify at the SDK boundary:** `From<aws_sdk_dynamodb::Error>` routes through `AppError::from_dynamo_code`, mapping `ValidationException` → 400 and logging the service code (an `aws_sdk_dynamodb::Error`'s bare `Display` is only `"unhandled error (ValidationException)"` — no operation, no table). This is safe as a blanket rule because everything reaching that `From` goes through the `shared::dynamo` helpers, which build requests purely from a caller-supplied key/item map and a fixed key-condition expression; the repos' *server-assembled* dynamic `update_item` expressions use `.map_err(AppError::internal)` instead, so a genuine expression bug still surfaces as a 500. Throttling, permissions, network and timeouts stay 500 in both layers. **Bounded AWS calls** — every AWS SDK client is built from one shared config (`shared::aws::base_config`) that sets a default operation timeout (per-attempt + overall) so a stalled call can't hang a request handler or an SQS worker indefinitely; the SQS clients override this with a longer, long-poll-safe timeout because the consumers receive with 20s long-polling (a per-attempt timeout shorter than the long-poll would cut every idle poll short).

#### 10.11 Link previews (Open Graph social unfurls)

Watch-page links unfurl as a rich card (title + description + large thumbnail + `Rewind` site name) in Slack/Discord/Twitter, instead of the site-wide default. Crawlers fetch raw HTML and read `<meta>` tags without running JavaScript, so the metadata must be **server-rendered** — but the rest of the app is client-rendered. The watch route is therefore the app's one server-rendered data path: `app/watch/[id]/page.tsx` is a thin **Server Component** that exports `generateMetadata` and renders the interactive UI in `WatchClient.tsx` (`"use client"`). Next.js resolves `generateMetadata` **synchronously into `<head>` for detected crawlers** (Slackbot, Twitterbot, etc.), so the dynamically-rendered page still unfurls correctly.

`generateMetadata` fetches catalog `GET /videos/{id}` **anonymously** (a crawler has no session, and catalog's `get_video` is unauthenticated) with `no-store` + a short timeout; any failure resolves to the generic card. The tag-building logic is a pure, unit-tested function (`frontend/lib/metadata.ts`), split from the fetch I/O at the page edge. Visibility gating: **public/unlisted → rich card** (unlisted is included because it's shareable by direct link — unfurling a deliberately-shared link matches intent, as YouTube does); **private / deleted / not-found → the generic `Rewind` card**, so an owner-only title or thumbnail never leaks to a channel. The card is `summary_large_image` when a thumbnail exists, else `summary`; `og:type` is `video.other`.

The `og:image` is the **public content-CDN thumbnail URL** — `https://cdn.<domain>/{thumbnail_key}`, built from the bare S3 key stored in `videos.thumbnail_url`. This reuses the existing CloudFront delivery path (the `videos` bucket is the CDN origin and public/unlisted objects are served unsigned), so it needs **no new infrastructure** and is a stable, cacheable, auth-free URL — unlike the short-lived presigned URL the player uses. **The player's stream/thumbnail fetching is unchanged**; link previews only *add* a public image URL for public/unlisted videos. `og:url` uses the frontend's apex origin. Both origins are supplied as build-time env (`NEXT_PUBLIC_SITE_URL`, `NEXT_PUBLIC_CDN_URL`) wired in `docker/Dockerfile.frontend` + `scripts/deploy.sh`. Scope is the rich summary card only; in-Slack *playback* (a `twitter:player`/`og:video` embed) was intentionally not built.

#### 10.12 Status

The MVP and all the post-MVP work above are built and live in both regions. **Remaining and future work — CI/CD, deep-canary enablement, Phase-4 resilience drills, and the deferred follow-ups — is tracked in `docs/TASKS.md`.**
