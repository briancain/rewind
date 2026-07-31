# infra/cdn — the content CDN: a single GLOBAL CloudFront distribution serving HLS/MP4 from the
# regional `videos` S3 buckets, plus its us-east-1 ACM cert, OAC, CORS policy, the videos-bucket
# policy granting the distribution access, and the `cdn.${domain}` DNS record.
#
# Why its own stack: CloudFront is global, but the buckets are per-region.
# Keeping the distribution in the per-region env would (a) collide when a second region's env tried
# to create the same `cdn.${domain}` distribution/record, and (b) make it awkward to add the second
# region's bucket as a failover origin. As a dedicated stack it reads each region's videos bucket via
# remote state and owns the cross-region origin group in one place.
#
# Dependency DAG (one-directional, no cycles): bootstrap -> global -> regional env(s) -> cdn.
# This stack reads the hosted zone from `global` and the videos bucket from each regional env, and
# owns the bucket policy (so the env never needs the distribution ARN — avoids a circular dependency).
#
# MULTI-REGION (expansion): add a second `data.terraform_remote_state.regional_use2`, a second
# `origin` block for that bucket, and wrap both origins in an `origin_group` (primary + failover) so
# `manifest_url` stays a single global `cdn.${domain}`. Today there is one region, so one origin.

terraform {
  required_version = ">= 1.5"

  backend "s3" {
    bucket         = "rewind-terraform-state"
    key            = "cdn/terraform.tfstate"
    region         = "us-west-2"
    dynamodb_table = "rewind-terraform-locks"
    encrypt        = true
    profile        = "rewind"
  }

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }
}

variable "aws_profile" {
  description = "AWS CLI profile name"
  type        = string
  default     = "rewind"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "dev"
}

# Secondary region for the CloudFront origin group. Empty = single origin (us-west-2 only). When set
# (e.g. "us-east-2"), a second origin (that region's videos bucket) is added and both are wrapped in
# an origin group with per-request 4xx/5xx failover, so `manifest_url` stays one global cdn.${domain}
# and content present in only one region is still served. Requires the secondary
# env's remote state to exist (apply the us-east-2 env first).
variable "secondary_region" {
  description = "Secondary region whose videos bucket joins the CloudFront origin group (empty = single origin)"
  type        = string
  default     = ""
}

# Default provider — used for the S3 bucket policy (the S3 control-plane API is global, so the
# provider region need not match the bucket region) and for the Route 53 record.
provider "aws" {
  region  = "us-west-2"
  profile = var.aws_profile

  default_tags {
    tags = {
      Project   = "rewind"
      ManagedBy = "terraform"
      Component = "cdn"
    }
  }
}

# CloudFront viewer certificates MUST live in us-east-1.
provider "aws" {
  alias   = "us_east_1"
  region  = "us-east-1"
  profile = var.aws_profile

  default_tags {
    tags = {
      Project   = "rewind"
      ManagedBy = "terraform"
      Component = "cdn"
    }
  }
}

# The secondary videos bucket lives in another region; PutBucketPolicy must be sent to that bucket's
# regional endpoint. Region comes from the variable, with a valid fallback so single-origin (empty
# secondary_region) still initializes — the resource using this alias is count-gated off in that case.
provider "aws" {
  alias   = "secondary"
  region  = var.secondary_region != "" ? var.secondary_region : "us-west-2"
  profile = var.aws_profile

  default_tags {
    tags = {
      Project   = "rewind"
      ManagedBy = "terraform"
      Component = "cdn"
    }
  }
}

data "terraform_remote_state" "global" {
  backend = "s3"
  config = {
    bucket  = "rewind-terraform-state"
    key     = "global/terraform.tfstate"
    region  = "us-west-2"
    profile = var.aws_profile
  }
}

# The home region's regional env (owns the primary videos bucket).
data "terraform_remote_state" "regional_usw2" {
  backend = "s3"
  config = {
    bucket  = "rewind-terraform-state"
    key     = "${var.environment}/us-west-2/terraform.tfstate"
    region  = "us-west-2"
    profile = var.aws_profile
  }
}

# The secondary region's regional env (owns the failover videos bucket). Only read when
# secondary_region is set — keeps the distribution single-origin and applyable until expansion.
data "terraform_remote_state" "regional_secondary" {
  count   = var.secondary_region != "" ? 1 : 0
  backend = "s3"
  config = {
    bucket  = "rewind-terraform-state"
    key     = "${var.environment}/${var.secondary_region}/terraform.tfstate"
    region  = "us-west-2"
    profile = var.aws_profile
  }
}

locals {
  domain     = data.terraform_remote_state.global.outputs.domain
  cdn_domain = "cdn.${local.domain}"
  zone_id    = data.terraform_remote_state.global.outputs.hosted_zone_id

  # Alert email for the global (us-east-1) CloudFront / Route 53 alarms — reused from the primary
  # env's tfvars via remote state so there's a single source of truth for the address.
  alert_email = data.terraform_remote_state.regional_usw2.outputs.alert_email

  primary_bucket_id            = data.terraform_remote_state.regional_usw2.outputs.videos_bucket_id
  primary_bucket_arn           = data.terraform_remote_state.regional_usw2.outputs.videos_bucket_arn
  primary_bucket_regional_fqdn = data.terraform_remote_state.regional_usw2.outputs.videos_bucket_regional_domain_name

  # Secondary (failover) origin, only when secondary_region is set.
  secondary_enabled              = var.secondary_region != ""
  secondary_bucket_id            = local.secondary_enabled ? data.terraform_remote_state.regional_secondary[0].outputs.videos_bucket_id : ""
  secondary_bucket_arn           = local.secondary_enabled ? data.terraform_remote_state.regional_secondary[0].outputs.videos_bucket_arn : ""
  secondary_bucket_regional_fqdn = local.secondary_enabled ? data.terraform_remote_state.regional_secondary[0].outputs.videos_bucket_regional_domain_name : ""
  secondary_origin_id            = "videos-s3-${var.secondary_region}"

  # Behaviors target the origin GROUP when a second origin exists, else the single primary origin.
  target_origin_id = local.secondary_enabled ? "videos-origin-group" : "videos-s3"
}

# --- ACM certificate (us-east-1, DNS-validated against the global hosted zone) ---
resource "aws_acm_certificate" "cdn" {
  provider          = aws.us_east_1
  domain_name       = local.cdn_domain
  validation_method = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "cdn_cert_validation" {
  for_each = {
    for dvo in aws_acm_certificate.cdn.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      type   = dvo.resource_record_type
      record = dvo.resource_record_value
    }
  }

  zone_id = local.zone_id
  name    = each.value.name
  type    = each.value.type
  records = [each.value.record]
  ttl     = 60
}

resource "aws_acm_certificate_validation" "cdn" {
  provider                = aws.us_east_1
  certificate_arn         = aws_acm_certificate.cdn.arn
  validation_record_fqdns = [for r in aws_route53_record.cdn_cert_validation : r.fqdn]
}

# --- Origin Access Control (CloudFront -> private S3 origin) ---
resource "aws_cloudfront_origin_access_control" "videos" {
  name                              = "rewind-${var.environment}-videos-oac"
  origin_access_control_origin_type = "s3"
  signing_behavior                  = "always"
  signing_protocol                  = "sigv4"
}

# --- CORS response headers so hls.js (cross-origin XHR from the watch app) can read the manifest ---
resource "aws_cloudfront_response_headers_policy" "hls_cors" {
  name = "rewind-${var.environment}-hls-cors"

  cors_config {
    access_control_allow_credentials = false

    access_control_allow_headers {
      items = ["*"]
    }
    access_control_allow_methods {
      items = ["GET", "HEAD", "OPTIONS"]
    }
    access_control_allow_origins {
      items = ["https://${local.domain}"]
    }

    origin_override = true
  }
}

resource "aws_cloudfront_distribution" "videos" {
  enabled         = true
  comment         = "rewind-${var.environment} HLS delivery"
  aliases         = [local.cdn_domain]
  price_class     = "PriceClass_100"
  is_ipv6_enabled = true

  # Primary origin: the us-west-2 videos bucket.
  origin {
    domain_name              = local.primary_bucket_regional_fqdn
    origin_id                = "videos-s3"
    origin_access_control_id = aws_cloudfront_origin_access_control.videos.id

    # Origin Shield (in the primary bucket's region) consolidates cache-fills through one regional
    # cache, reducing origin load for the long tail. At expansion it
    # also smooths the origin-group failover path.
    origin_shield {
      enabled              = true
      origin_shield_region = "us-west-2"
    }
  }

  # Secondary (failover) origin: the secondary region's videos bucket. Reuses the same OAC (a signing
  # config, not bucket-specific). Only present when secondary_region is set.
  dynamic "origin" {
    for_each = local.secondary_enabled ? [1] : []
    content {
      domain_name              = local.secondary_bucket_regional_fqdn
      origin_id                = local.secondary_origin_id
      origin_access_control_id = aws_cloudfront_origin_access_control.videos.id

      origin_shield {
        enabled              = true
        origin_shield_region = var.secondary_region
      }
    }
  }

  # Origin group: try the primary bucket first, fail over to the secondary on 4xx/5xx so content
  # present in only one region (e.g. not yet CRR-replicated) is still served. Only present when a
  # secondary origin exists.
  dynamic "origin_group" {
    for_each = local.secondary_enabled ? [1] : []
    content {
      origin_id = "videos-origin-group"

      failover_criteria {
        status_codes = [403, 404, 500, 502, 503, 504]
      }

      member {
        origin_id = "videos-s3"
      }
      member {
        origin_id = local.secondary_origin_id
      }
    }
  }

  default_cache_behavior {
    target_origin_id       = local.target_origin_id
    viewer_protocol_policy = "redirect-to-https"
    allowed_methods        = ["GET", "HEAD", "OPTIONS"]
    cached_methods         = ["GET", "HEAD"]

    # AWS-managed policies: CachingOptimized (cache) + CORS-S3Origin (forward Origin to S3).
    cache_policy_id            = "658327ea-f89d-4fab-a63d-7e88639e58f6"
    origin_request_policy_id   = "88a5eaf4-2fd4-4709-b370-b4c650ea3fcf"
    response_headers_policy_id = aws_cloudfront_response_headers_policy.hls_cors.id
  }

  restrictions {
    geo_restriction {
      restriction_type = "none"
    }
  }

  viewer_certificate {
    acm_certificate_arn      = aws_acm_certificate_validation.cdn.certificate_arn
    ssl_support_method       = "sni-only"
    minimum_protocol_version = "TLSv1.2_2021"
  }
}

# --- Allow only this distribution to read each region's videos bucket (OAC) ---
# Owned here (not in the env) so the env never needs the distribution ARN — keeps the env -> cdn
# dependency one-directional.
resource "aws_s3_bucket_policy" "videos_cloudfront_usw2" {
  bucket = local.primary_bucket_id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontOAC"
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = "s3:GetObject"
      Resource  = "${local.primary_bucket_arn}/*"
      Condition = {
        StringEquals = { "AWS:SourceArn" = aws_cloudfront_distribution.videos.arn }
      }
    }]
  })
}

# Same grant for the secondary region's videos bucket (only when the origin group is enabled).
resource "aws_s3_bucket_policy" "videos_cloudfront_secondary" {
  count    = local.secondary_enabled ? 1 : 0
  provider = aws.secondary
  bucket   = local.secondary_bucket_id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Sid       = "AllowCloudFrontOAC"
      Effect    = "Allow"
      Principal = { Service = "cloudfront.amazonaws.com" }
      Action    = "s3:GetObject"
      Resource  = "${local.secondary_bucket_arn}/*"
      Condition = {
        StringEquals = { "AWS:SourceArn" = aws_cloudfront_distribution.videos.arn }
      }
    }]
  })
}

# --- DNS: cdn.${domain} -> distribution (takes precedence over the *.${domain} ALB wildcard) ---
resource "aws_route53_record" "cdn" {
  zone_id = local.zone_id
  name    = local.cdn_domain
  type    = "A"

  alias {
    name                   = aws_cloudfront_distribution.videos.domain_name
    zone_id                = aws_cloudfront_distribution.videos.hosted_zone_id
    evaluate_target_health = false
  }
}

# --- Alerting for the global content CDN (CloudFront metrics are emitted only in us-east-1) ---
# CloudFront metrics are GLOBAL, published only in us-east-1, so their alarms — and the SNS topic they
# notify — must live in us-east-1 (the per-region observability topics can't be targeted from a
# us-east-1 alarm). This topic reuses the env's alert email.
# NOTE: applying this creates a NEW SNS email subscription — you must click the confirmation email
# once before alerts deliver. (This same topic is reused by the Route 53 health-check alarms.)
resource "aws_sns_topic" "global_alerts" {
  provider = aws.us_east_1
  name     = "rewind-${var.environment}-global-alerts"
}

resource "aws_sns_topic_subscription" "global_alerts_email" {
  provider  = aws.us_east_1
  topic_arn = aws_sns_topic.global_alerts.arn
  protocol  = "email"
  endpoint  = local.alert_email
}

variable "cloudfront_5xx_error_rate_threshold" {
  description = "CloudFront 5xxErrorRate (%) over 5 min above which the alarm fires. Edge/origin 5xx breaks playback for viewers even though streaming returned a 200 manifest URL."
  type        = number
  default     = 5
}

variable "cloudfront_total_error_rate_threshold" {
  description = "CloudFront TotalErrorRate (%, 4xx+5xx) over 5 min above which the alarm fires. Higher than the 5xx threshold since some 4xx (404s during origin-group failover) are expected."
  type        = number
  default     = 25
}

# The single biggest playback blind spot: video bytes (HLS/MP4/thumbnails) reach viewers via this
# distribution, not the ALB, so an edge/origin 5xx is invisible to every ALB/per-service alarm.
# 5xxErrorRate + TotalErrorRate are default (free) CloudFront metrics; dimension Region = "Global".
# (OriginLatency would require the paid additional-metrics monitoring subscription — omitted, matching
# the project's cost stance on paid CloudWatch/WAF features.)
resource "aws_cloudwatch_metric_alarm" "cloudfront_5xx" {
  provider            = aws.us_east_1
  alarm_name          = "rewind-${var.environment}-cdn-5xx-error-rate"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "5xxErrorRate"
  namespace           = "AWS/CloudFront"
  period              = 300
  statistic           = "Average"
  threshold           = var.cloudfront_5xx_error_rate_threshold
  alarm_description   = "CloudFront 5xx error rate > ${var.cloudfront_5xx_error_rate_threshold}% — video delivery (HLS/MP4/thumbnails) is failing at the edge/origin"
  alarm_actions       = [aws_sns_topic.global_alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    DistributionId = aws_cloudfront_distribution.videos.id
    Region         = "Global"
  }
}

resource "aws_cloudwatch_metric_alarm" "cloudfront_total_error" {
  provider            = aws.us_east_1
  alarm_name          = "rewind-${var.environment}-cdn-total-error-rate"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "TotalErrorRate"
  namespace           = "AWS/CloudFront"
  period              = 300
  statistic           = "Average"
  threshold           = var.cloudfront_total_error_rate_threshold
  alarm_description   = "CloudFront total error rate > ${var.cloudfront_total_error_rate_threshold}% (4xx+5xx) — broad content-delivery failure"
  alarm_actions       = [aws_sns_topic.global_alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    DistributionId = aws_cloudfront_distribution.videos.id
    Region         = "Global"
  }
}

output "cdn_domain" {
  value = local.cdn_domain
}

output "distribution_id" {
  value = aws_cloudfront_distribution.videos.id
}
