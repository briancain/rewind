# infra/observability — global multi-region dashboard

The single, account-wide CloudWatch **dashboard** for the platform.

## Why this is a global stack (not in the per-region module)

CloudWatch **dashboards are a global resource** — one name per account, visible from every region.
The per-region `modules/observability` originally created the dashboard with a region-free name
(`rewind-dev`), so standing up a second region made both region stacks manage the *same* dashboard
and clobber each other on every apply. The fix is to own the dashboard **once**, here, and scope
each widget to its region via the widget's `region` property.

**Regional** observability — alarms, the SNS alert topic, log config — correctly stays in
`modules/observability` (those are region-specific resources). Only the global dashboard lives here.

## How it works

- Data-driven by `var.regions` (default `["us-west-2", "us-east-2"]`), in display order.
- For each region it reads that region's `dev/<region>` Terraform state (remote state) for the
  `alb_arn_suffix` output (the CloudWatch `LoadBalancer` dimension), and derives the region-free
  resource names (queues, pipes, OpenSearch domain) by convention.
- Renders one stacked band of widgets per region — a single active/active pane.

## Apply

```
cd infra/observability
terraform init
terraform apply
```

**Order:** apply *after* the target regions' `dev/<region>` envs exist (this stack reads their
state). Adding/removing a region = edit `regions` and re-apply. Works for 1..N regions.
