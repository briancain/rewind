# infra/environments/dev/us-west-2

Full dev environment for the region: VPC, EKS, ECR, S3, SQS, OpenSearch, MediaConvert, EventBridge
Pipes, IRSA, WAF, CloudWatch alarms, Ingress, DNS. The DynamoDB tables are **not** here — they're
owned by the region-neutral `infra/data` root and read from its state.

## Fresh Start (multi-pass apply)

**Prerequisite:** apply `infra/data` first. That region-neutral root owns the DynamoDB tables (the
Global Table origin); this env only *reads* them via `terraform_remote_state`, so a plan here fails
until that stack exists.

Two ordering constraints force a multi-pass apply on a fresh deploy:
- The Kubernetes provider can't connect to EKS during plan until the cluster exists.
- The latency DNS records + Route 53 health check can't be created until the ALB exists (the ALB
  is provisioned by the AWS LB controller from the Ingress).

```bash
terraform init

# 1. Core infra (VPC, EKS, ECR, regional data, IRSA, namespace) — ~15 min
terraform apply -auto-approve \
  -target=module.vpc \
  -target=module.eks \
  -target=module.ecr \
  -target=module.regional_data \
  -target=module.irsa \
  -target=kubernetes_namespace.rewind
# If the namespace step fails with "Unauthorized", the EKS access entry hasn't propagated yet —
# just re-run the same command (the cluster is up; it succeeds on retry).

# 2. Kubernetes manifests (IngressClass, NodePool, Ingress) — the Ingress provisions the ALB (~2 min)
terraform apply -auto-approve \
  -target=kubernetes_manifest.ingress_class_params \
  -target=kubernetes_manifest.ingress_class \
  -target=kubernetes_manifest.nodepool_arm64 \
  -target=kubernetes_manifest.ingress

# 3. Full apply — creates the WAF association, the alarms, and the latency DNS records + health
#    check (all need the ALB, which now exists) AND writes ALL outputs. This MUST succeed before
#    step 4: deploy.sh reads the outputs, and -target applies in steps 1–2 do not write them
#    (e.g. `region` would be missing).
terraform apply -auto-approve

# 4. Deploy services + the two reconcile CronJobs + frontend + canary, then seed OpenSearch
../../../../scripts/deploy.sh
../../../../scripts/reindex.sh

# 5. Content CDN (separate stack — CloudFront over the videos bucket). Reads this env's outputs.
#    Add `-var 'secondary_region=us-east-2'` once the second region exists, for the origin group.
( cd ../../../cdn && terraform init && terraform apply -auto-approve )

# 6. Global multi-region CloudWatch dashboard (account-global — owned once, outside this env).
( cd ../../../observability && terraform init && terraform apply -auto-approve )
```

## Subsequent applies

Once all resources exist, `terraform apply` works in a single shot (and `infra/cdn` too).

## First-time setup (tfvars)

Account/personal values live in a gitignored `terraform.tfvars` (not committed). Copy the example
and set your own before the first apply:

```bash
cp terraform.tfvars.example terraform.tfvars
# then edit terraform.tfvars — set admin_role_arn (your account's admin role) and alert_email
```

`admin_role_arn` and `alert_email` are **required** (no defaults) so nothing account-specific is
baked into the committed config.

## Variables

| Name | Description | Default |
|------|-------------|---------|
| `aws_profile` | AWS CLI profile | `rewind` |
| `region` | AWS region | `us-west-2` |
| `environment` | Environment name | `dev` |
| `admin_role_arn` | IAM role for EKS admin access (set in `terraform.tfvars`) | **required** |
| `alert_email` | Email subscribed to the SNS alarm topic (set in `terraform.tfvars`) | **required** |
| `enable_replication` | Enable bidirectional S3 CRR (peer derived from topology) | `true` |
