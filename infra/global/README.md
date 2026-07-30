# infra/global

Region-agnostic resources: Route 53 hosted zone + ACM wildcard certificate.

## Fresh Start (first-time deploy)

ACM DNS validation requires the hosted zone to be resolvable, which requires NS
delegation from the parent account. This creates a two-step process:

```bash
# 1. Create the hosted zone and cert (but skip validation wait)
terraform init
terraform apply -auto-approve \
  -target=aws_route53_zone.main \
  -target=aws_acm_certificate.main

# 2. Get the nameservers
terraform output nameservers

# 3. Add NS delegation in your parent account (the account that owns the parent domain)
aws route53 change-resource-record-sets \
  --hosted-zone-id <PARENT_ZONE_ID> \
  --profile <PARENT_PROFILE> \
  --change-batch '{
    "Changes": [{"Action": "UPSERT", "ResourceRecordSet": {
      "Name": "<YOUR_DOMAIN>", "Type": "NS", "TTL": 300,
      "ResourceRecords": [{"Value": "ns-xxx"}, ...]
    }}]
  }'

# 4. Full apply (creates validation records, waits for ACM to confirm ~5 min)
terraform apply -auto-approve
```

## Subsequent applies

Once the NS delegation exists, `terraform apply` works in a single shot.

## First-time setup (tfvars)

Account/personal values live in a gitignored `terraform.tfvars` (not committed). Copy the example
and set your own before the first apply:

```bash
cp terraform.tfvars.example terraform.tfvars
# then edit terraform.tfvars — set `domain` to your own root domain
```

Every other stack reads the domain from this stack's `domain` output, so it's set in **one** place.

## Variables

| Name | Description | Default |
|------|-------------|---------|
| `domain` | Root domain for the platform (set in `terraform.tfvars`) | `watch.example.com` |
| `aws_profile` | AWS CLI profile | `rewind` |
