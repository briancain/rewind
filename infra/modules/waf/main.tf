# Regional WAFv2 web ACL associated with this region's ALB (the one the EKS Auto Mode ALB controller
# creates from the Ingress). Association is done in Terraform against the discovered ALB ARN rather
# than via an Ingress annotation, so it doesn't depend on EKS-Auto annotation support and is explicit
# in state. REGIONAL scope (ALB); the provider region is inherited from the calling env root.
#
# Default action is allow; the rules below block abusive/known-bad traffic. The dedicated paid
# AWSManagedRulesBotControlRuleSet is intentionally NOT included (cost vs. value for an invite-only
# demo); rules 2-4 are free AWS managed groups and the rate-based rule covers volumetric abuse.

variable "name" {
  description = "Region-free resource name prefix (e.g. rewind-dev). REGIONAL WAF names may repeat across regions."
  type        = string
}

variable "alb_arn" {
  description = "ARN of the ALB to associate the web ACL with."
  type        = string
}

variable "rate_limit" {
  description = "Max requests per 5-minute window per source IP before the rate-based rule blocks."
  type        = number
  default     = 2000
}

resource "aws_wafv2_web_acl" "this" {
  name        = "${var.name}-alb"
  scope       = "REGIONAL"
  description = "Rate limiting + AWS managed protections for the ${var.name} regional ALB."

  default_action {
    allow {}
  }

  # 1. Rate limiting — block a source IP exceeding rate_limit requests per 5-minute window.
  rule {
    name     = "rate-limit"
    priority = 0

    action {
      block {}
    }

    statement {
      rate_based_statement {
        limit              = var.rate_limit
        aggregate_key_type = "IP"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-rate-limit"
      sampled_requests_enabled   = true
    }
  }

  # 2. Amazon IP reputation list — known malicious / bot source IPs + reconnaissance (free).
  rule {
    name     = "ip-reputation"
    priority = 1

    override_action {
      none {}
    }

    statement {
      managed_rule_group_statement {
        vendor_name = "AWS"
        name        = "AWSManagedRulesAmazonIpReputationList"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-ip-reputation"
      sampled_requests_enabled   = true
    }
  }

  # 3. Core rule set — common exploit protections, OWASP-style (free).
  rule {
    name     = "common"
    priority = 2

    override_action {
      none {}
    }

    statement {
      managed_rule_group_statement {
        vendor_name = "AWS"
        name        = "AWSManagedRulesCommonRuleSet"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-common"
      sampled_requests_enabled   = true
    }
  }

  # 4. Known bad inputs — request patterns tied to known vulns/exploits (free).
  rule {
    name     = "known-bad-inputs"
    priority = 3

    override_action {
      none {}
    }

    statement {
      managed_rule_group_statement {
        vendor_name = "AWS"
        name        = "AWSManagedRulesKnownBadInputsRuleSet"
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-known-bad-inputs"
      sampled_requests_enabled   = true
    }
  }

  visibility_config {
    cloudwatch_metrics_enabled = true
    metric_name                = "${var.name}-alb-waf"
    sampled_requests_enabled   = true
  }

  tags = {
    Name = "${var.name}-alb-waf"
  }
}

resource "aws_wafv2_web_acl_association" "this" {
  resource_arn = var.alb_arn
  web_acl_arn  = aws_wafv2_web_acl.this.arn
}

output "web_acl_arn" {
  value = aws_wafv2_web_acl.this.arn
}

output "web_acl_name" {
  value = aws_wafv2_web_acl.this.name
}
