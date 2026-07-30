# dev/us-east-2 — secondary region environment

The active/active secondary region. Same modules as `dev/us-west-2`, differing
only by: backend key (`dev/us-east-2`), region (`us-east-2`), VPC CIDR (`10.1.0.0/16`), its **own
regional ALB ACM cert** (ACM is regional; the global stack's cert is us-west-2-only), region-local
DynamoDB replica ARNs for IRSA, and its videos stream sourced from this region's **local Global
Table replica** (not the home stream).

**Live** since 2026-06-18. Bring-up order: see the multi-region section of `docs/DESIGN.md`.

## First-time setup (tfvars)

Same as the us-west-2 env: copy `terraform.tfvars.example` to a gitignored `terraform.tfvars` and
set the required `admin_role_arn` and `alert_email` before the first apply.

```bash
cp terraform.tfvars.example terraform.tfvars
# then edit terraform.tfvars
```


**Why region-local replica stream + table ARNs:** under Global Tables each region has its own stream
and region-qualified ARNs, so this region's Pipes (search + cascade-cleanup) consume the *local*
replica stream and its IRSA policies use *local* ARNs — each region converges independently, with no
cross-region calls.
