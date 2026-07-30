# Rewind Service

Brian Cain 2026

Video streaming platform — YouTube/Netflix hybrid built with Rust microservices and Next.js.

## Architecture

- **Backend:** 9 Rust crates on EKS — 6 request-serving microservices (identity, video-catalog, upload, streaming, social, search), 2 queue-driven workers (`transcode`, which orchestrates MediaConvert, and `delete-cleanup`, the cascade-delete worker — both expose only `/health`), and a `canary` integration-test workload
- **Frontend:** Next.js + TypeScript + Tailwind CSS
- **Infrastructure:** AWS (EKS, DynamoDB, S3, SQS, EventBridge Pipes, MediaConvert, OpenSearch, CloudFront, CloudWatch)
- **Multi-region:** Active/active across us-west-2 + us-east-2 — **live** (DynamoDB Global Tables, bidirectional S3 CRR, CloudFront origin group, Route 53 latency + health-check failover)

See [docs/DESIGN.md](docs/DESIGN.md) for architecture decisions, and [docs/TASKS.md](docs/TASKS.md) for the backlog.
The system diagram lives in [`docs/architecture.svg`](docs/architecture.svg) (source:
[`docs/architecture.mmd`](docs/architecture.mmd)).

## Local Development

**Prerequisites:** Finch (or Docker), Rust, Node.js 22+, AWS CLI, ffmpeg

```bash
# Start everything (infra, backend, frontend) in one command:
./scripts/dev.sh

# Open http://localhost:3000
# Press Ctrl+C to stop services (containers stay running for fast restart)

# Full teardown (including containers):
./scripts/local-stop.sh
```

`dev.sh` handles: Finch containers → readiness checks → DynamoDB tables/S3 buckets/SQS queues → Rust build → the 7 locally-run backend services → Next.js frontend. Logs are color-coded per service. (`delete-cleanup` and `canary` are cloud-only and aren't started locally.)

**Note:** Registration requires an invite code. Generate one locally:
```bash
./scripts/invite.sh
```

**Seed local content.** To exercise browse / surf / watch without uploading through the UI, seed the
local stack from the `testdata` catalog (uploads the source videos to LocalStack S3, generates
thumbnails via ffmpeg, and writes the catalog + channel rows):
```bash
./scripts/seed-local.sh
```
Source videos are git-ignored — see [`testdata/README.md`](testdata/README.md). `dev.sh` creates the
tables/buckets/queues for you, but `./scripts/local-setup.sh` does it standalone if you only want the
data plane up (`finch compose up -d dynamodb-local localstack`).

**Services (local ports):**

| Service | Port | Notes |
|---------|------|-------|
| identity | 8080 | SES disabled locally |
| video-catalog | 8081 | |
| upload | 8082 | Presigned URLs via LocalStack S3 |
| streaming | 8083 | Presigns video/thumbnail from LocalStack |
| social | 8084 | |
| search | 8085 | OpenSearch on :9200 |
| transcode | 8086 | Local: copies raw→videos bucket + ffmpeg thumbnail. Cloud: AWS MediaConvert |
| frontend | 3000 | Next.js dev server |

## Deployment

**Prerequisites:** Finch, Helm, kubectl, Terraform ≥ 1.5, AWS CLI with `rewind` profile configured

**Quick version (existing infra):**

```bash
./scripts/deploy.sh [TAG]
```

**Check platform health:**

```bash
./scripts/status.sh                 # us-west-2 (default)
REGION=us-east-2 ./scripts/status.sh
```

**Deploy a single service (fast iteration):**

```bash
# Read terraform outputs once
TF_OUT=$(cd infra/environments/dev/us-west-2 && terraform output -json)
tf() { echo "$TF_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)$1)"; }

# Build, push, and deploy one service (e.g. transcode)
SERVICE=transcode
TAG=$(git rev-parse --short HEAD)
REPO=$(tf "['ecr_repository_urls']['value']['$SERVICE']")

finch build --build-arg SERVICE_NAME=$SERVICE -t "${REPO}:${TAG}" -f docker/Dockerfile .
finch push "${REPO}:${TAG}"
helm upgrade $SERVICE helm/rewind-service -f helm/values/${SERVICE}.yaml \
  --namespace rewind --set image.repository="$REPO" --set image.tag="$TAG" \
  --set serviceAccount.roleArn="$(tf "['service_role_arns']['value']['$SERVICE']")" \
  [--set "env[N].name=...,env[N].value=..." as needed] \
  --wait --timeout 120s
```

For frontend, use `docker/Dockerfile.frontend` with `--build-arg NEXT_PUBLIC_*` flags (see deploy.sh).

**Forking this project (first-time tfvars):**

Account/personal values are not committed — each Terraform root reads them from a gitignored
`terraform.tfvars`. Before the first apply, copy the example in each root and set your own values:

```bash
cp infra/global/terraform.tfvars.example                  infra/global/terraform.tfvars
cp infra/environments/dev/us-west-2/terraform.tfvars.example infra/environments/dev/us-west-2/terraform.tfvars
cp infra/environments/dev/us-east-2/terraform.tfvars.example infra/environments/dev/us-east-2/terraform.tfvars
# edit each: `domain` (global), `admin_role_arn` + `alert_email` (env roots)
```

> **Known gap (forks):** the Terraform `backend "s3"` blocks still hardcode the state bucket
> (`rewind-terraform-state`), lock table, and `rewind` AWS profile. Backend blocks can't take
> variables, and the bucket name is globally unique, so a fork must change those by hand for now —
> tracked in `docs/TASKS.md` (partial-backend-config cleanup). The `rewind` project-name prefix and
> profile default are intentionally kept.

**Fresh start (new account/from scratch):**

Terraform roots apply in dependency order — each reads the previous ones' state:

```bash
# 1. Bootstrap Terraform state backend
cd infra/bootstrap && terraform init && terraform apply

# 2. DNS + TLS certificate
cd infra/global && terraform init && terraform apply
# → Then add NS delegation in your parent account (see infra/global/README.md)

# 3. DynamoDB tables — the region-neutral stack that OWNS all 11 tables (the Global Table origin).
#    Only depends on bootstrap, but must precede BOTH regional envs: us-west-2 reads the table ARNs
#    from this stack's state, while us-east-2 derives its region-qualified replica ARNs by convention
#    and looks its local replica up live — so that region's EventBridge Pipes can't resolve their
#    stream until the replica exists. `replica_regions` defaults to [] (single-region) — set it for
#    active/active.
cd infra/data && terraform init && terraform apply -var 'replica_regions=["us-east-2"]'

# 4. Primary region (VPC, EKS, ECR, S3, SQS, OpenSearch, IRSA, WAF, alarms, Ingress + DNS)
cd infra/environments/dev/us-west-2 && terraform init && terraform apply
# → Fresh deploys are multi-pass: the k8s provider needs the cluster to exist, and the latency
#   DNS records need the ALB (created by the Ingress). End on a full `terraform apply` so that
#   ALL outputs are written before the deploy step (deploy.sh reads them; a -target apply won't
#   write them). See infra/environments/dev/us-west-2/README.md.

# 5. Build, push, deploy: all services + the two reconcile CronJobs + frontend + canary
./scripts/deploy.sh                    # REGION defaults to us-west-2
./scripts/reindex.sh                   # seed this region's OpenSearch index

# 6. Second region — same modules, same multi-pass shape as step 4
cd infra/environments/dev/us-east-2 && terraform init && terraform apply
REGION=us-east-2 ./scripts/deploy.sh
REGION=us-east-2 ./scripts/reindex.sh
# → Objects that predate the new region's CRR rule need a one-time backfill:
#   ./scripts/backfill-replication.sh

# 7. Content CDN — CloudFront over the videos buckets + cdn.<domain> DNS. `secondary_region`
#    defaults to "" (single origin); set it to add the failover origin group.
cd infra/cdn && terraform init && terraform apply -var 'secondary_region=us-east-2'

# 8. Global multi-region CloudWatch dashboard (account-global, so owned once here).
#    Applies last — it reads each region's env state for the ALB dimension.
cd infra/observability && terraform init && terraform apply
```

`deploy.sh` reads all resource values (ECR URLs, table names, bucket names, queue URLs, IAM role ARNs, domain) from Terraform outputs at runtime — nothing is hardcoded. Select the target region with the `REGION` env var (default `us-west-2`), e.g. `REGION=us-east-2 ./scripts/deploy.sh`.

**Detailed guides:**
- [`infra/global/README.md`](infra/global/README.md) — DNS hosted zone + ACM cert setup
- [`infra/environments/dev/us-west-2/README.md`](infra/environments/dev/us-west-2/README.md) — Full environment provisioning
- [`infra/observability/README.md`](infra/observability/README.md) — Why the dashboard is a global stack

**Infrastructure layout:**

```
infra/
├── bootstrap/              # S3 state bucket + DynamoDB lock table (apply first)
├── modules/
│   ├── vpc/                # VPC, subnets, NAT, VPC endpoints
│   ├── eks/                # EKS Auto Mode cluster, IAM roles, OIDC provider
│   ├── ecr/                # Container registries per service
│   ├── global-data/        # DynamoDB tables + Global Table replicas
│   ├── regional-data/      # S3, SQS, OpenSearch, MediaConvert, EventBridge Pipes (per region)
│   ├── irsa/               # Per-service IAM roles for pod-level AWS access
│   ├── waf/                # Regional WAFv2 web ACL for the ALB (rate limit + managed rules)
│   └── observability/      # Regional CloudWatch alarms + SNS email alerts
├── data/                   # Region-neutral root: OWNS the DynamoDB tables (Global Table origin)
├── environments/
│   └── dev/                # Per-region roots: us-west-2 (primary) + us-east-2 (active/active)
├── cdn/                    # Global CloudFront origin group over both regions' videos buckets
├── observability/          # Global multi-region CloudWatch dashboard
└── global/                 # DNS hosted zone + ACM cert (region-agnostic)
```

**Naming convention:** DynamoDB tables + regional resources (EKS cluster, OpenSearch, SQS) use a
region-free prefix (`rewind-dev-*`) — tables because Global Table replicas share one name, regional
resources because they live in separate per-region namespaces. IAM roles (account-global) are
region-qualified (`rewind-dev-us-west-2-*`) so a second region's roles don't collide.

**Known gaps:**
- NS delegation from parent hosted zone is a manual step. See `infra/global/README.md`.
- No automated CD yet — deploys run from a laptop via `deploy.sh`. (CI does run on every push — see [Continuous Integration](#continuous-integration).)

## Operational scripts

| Script | Purpose |
|--------|---------|
| `scripts/status.sh` | Platform health snapshot (`REGION` selects the region) |
| `scripts/invite.sh` | Generate registration invite codes |
| `scripts/reindex.sh` | Seed/rebuild the OpenSearch index (in-cluster Job) |
| `scripts/canary.sh` | Integration canary: `setup` / `shallow` / `deep` / `enable` / `disable` (CronJobs deploy **suspended** — schedules off until enabled) |
| `scripts/redrive-transcode.sh` | Manually re-drive a video stranded in `processing` — the operator response to the `transcode-stuck-processing` alarm (the reconciler is detect + alarm only). Run it from the region that owns the raw upload |
| `scripts/backfill-replication.sh` | One-time S3 Batch Replication job to seed objects that predate a new region's CRR rule |
| `scripts/admin-reset-password.sh` | Break-glass password reset |

## Observability

A global multi-region CloudWatch dashboard and per-region alarms (infra health, customer experience, search-index sync, cascade cleanup, replication, and the canary) route to an SNS email topic; container logs ship via the CloudWatch Observability addon. The [canary](docs/DESIGN.md) validates the live user journey end-to-end — the shallow tier runs on schedule in both regions (staggered), with the deep tier suspended pending vetting.

## Continuous Integration

GitHub Actions (`.github/workflows/ci.yml`) runs on every push and PR:

- **Rust** — `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --all` (against LocalStack / OpenSearch / DynamoDB Local service containers)
- **Frontend** — lint, type-check, tests
- **Terraform** — `fmt -check` + `validate`
- **Helm** — `lint` + `template` (both charts)

No automated deploy (CD) yet — see [Deployment](#deployment).

## Tests

```bash
# Backend
cd services && cargo test --all -- --test-threads=1

# Frontend
cd frontend && npm test
```
