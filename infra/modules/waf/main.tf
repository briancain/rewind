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

variable "auth_rate_limit" {
  description = "Max POSTs to /login + /register per 5-minute window per source IP before the auth rate-based rule blocks. Deliberately far below var.rate_limit: that limit is sized for volumetric abuse and is roughly an order of magnitude above what one IP needs to run an effective credential / invite-code guessing loop, so auth needs its own much tighter budget. Kept as a SEPARATE rule rather than lowering the global limit, because the global limit also governs ordinary read traffic arriving from behind a shared NAT or corporate egress IP."
  type        = number
  default     = 100
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

  # 5. Auth-scoped rate limiting — a much tighter per-IP budget for the two credential endpoints.
  #
  # Rule 1's 2000/5-min is sized for volumetric abuse, so a single IP can guess invite codes or
  # passwords at a few hundred requests per window and never come close to it — invisible to every
  # block-based alarm and only visible after the fact as an auth-4xx spike. This caps that specific
  # behaviour without touching read traffic, which is why it is a separate rule instead of a lower
  # global limit (many legitimate users can share one NAT/corporate egress IP).
  #
  # The scope-down MUST require POST. The frontend serves `GET /login` as a page, so matching the
  # path alone would count ordinary page views toward the budget and could 403 real users behind a
  # shared egress IP. Only the JSON POSTs that actually attempt a credential are counted.
  #
  # Placed last so the existing rules keep their priorities (renumbering managed rule groups is
  # needless churn). A request blocked by an earlier rule never reaches this one and so is not
  # counted, which is fine: the managed groups almost never match a well-formed auth POST.
  #
  # Both path matches use a LOWERCASE transformation so a cased variant can't slip the rule; such a
  # request 404s at the service anyway, and rate-limiting it is harmless.
  rule {
    name     = "auth-rate-limit"
    priority = 4

    action {
      block {}
    }

    statement {
      rate_based_statement {
        limit              = var.auth_rate_limit
        aggregate_key_type = "IP"

        scope_down_statement {
          and_statement {
            statement {
              byte_match_statement {
                search_string         = "post"
                positional_constraint = "EXACTLY"

                field_to_match {
                  method {}
                }

                text_transformation {
                  priority = 0
                  type     = "LOWERCASE"
                }
              }
            }

            statement {
              or_statement {
                statement {
                  byte_match_statement {
                    search_string         = "/login"
                    positional_constraint = "EXACTLY"

                    field_to_match {
                      uri_path {}
                    }

                    text_transformation {
                      priority = 0
                      type     = "LOWERCASE"
                    }
                  }
                }

                statement {
                  byte_match_statement {
                    search_string         = "/register"
                    positional_constraint = "EXACTLY"

                    field_to_match {
                      uri_path {}
                    }

                    text_transformation {
                      priority = 0
                      type     = "LOWERCASE"
                    }
                  }
                }
              }
            }
          }
        }
      }
    }

    visibility_config {
      cloudwatch_metrics_enabled = true
      metric_name                = "${var.name}-auth-rate-limit"
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

# --- Request logging ---------------------------------------------------------------------------
#
# Without this, the only record of who sent a request is `wafv2:GetSampledRequests`, which returns a
# *sample* and retains just 3 hours — so any abuse investigation started more than a few hours after
# the fact cannot attribute traffic to a source at all. Logging every evaluated request (not only
# blocked ones: the traffic worth attributing is usually what the default-allow action let through)
# gives client IP, method, URI, headers, country and rule matches, queryable in Logs Insights for the
# retention window.
#
# Volume note: this logs health checks too (Route 53 probes are the bulk of steady-state traffic).
# WAF's logging_filter can only match on action or rule label, not URI, so they can't be excluded;
# retention is kept short instead to bound the cost rather than sampling and losing the signal.

variable "log_retention_days" {
  description = "Retention for the WAF request log group. Long enough to investigate an incident found days later; short enough to bound the cost of logging every request."
  type        = number
  default     = 14
}

data "aws_caller_identity" "current" {}
data "aws_region" "current" {}

resource "aws_cloudwatch_log_group" "waf" {
  # WAFv2 requires a CloudWatch destination log group name to begin with `aws-waf-logs-`.
  name              = "aws-waf-logs-${var.name}-alb"
  retention_in_days = var.log_retention_days

  tags = {
    Name = "aws-waf-logs-${var.name}-alb"
  }
}

# WAF delivers through the CloudWatch Logs delivery service, which needs an explicit resource policy
# when the logging configuration is created via the API (the console writes one implicitly). Scoped
# to this log group, and confused-deputy-guarded on the source account/ARN.
data "aws_iam_policy_document" "waf_logs" {
  statement {
    effect = "Allow"

    principals {
      type        = "Service"
      identifiers = ["delivery.logs.amazonaws.com"]
    }

    actions   = ["logs:CreateLogStream", "logs:PutLogEvents"]
    resources = ["${aws_cloudwatch_log_group.waf.arn}:*"]

    condition {
      test     = "StringEquals"
      variable = "aws:SourceAccount"
      values   = [data.aws_caller_identity.current.account_id]
    }

    condition {
      test     = "ArnLike"
      variable = "aws:SourceArn"
      values   = ["arn:aws:logs:${data.aws_region.current.name}:${data.aws_caller_identity.current.account_id}:*"]
    }
  }
}

resource "aws_cloudwatch_log_resource_policy" "waf_logs" {
  policy_name     = "${var.name}-waf-logs"
  policy_document = data.aws_iam_policy_document.waf_logs.json
}

resource "aws_wafv2_web_acl_logging_configuration" "this" {
  resource_arn = aws_wafv2_web_acl.this.arn
  # WAF rejects the `:*` stream suffix on a log group destination. The AWS provider already strips it
  # from `.arn`; trimming is belt-and-braces so a provider change can't break the apply.
  log_destination_configs = [trimsuffix(aws_cloudwatch_log_group.waf.arn, ":*")]

  # WAF logs full request headers. Redact the two that carry credentials so a session bearer token
  # can never be replayed out of CloudWatch Logs — the log is for attribution, not for secrets.
  redacted_fields {
    single_header {
      name = "authorization"
    }
  }

  redacted_fields {
    single_header {
      name = "cookie"
    }
  }

  depends_on = [aws_cloudwatch_log_resource_policy.waf_logs]
}

output "web_acl_arn" {
  value = aws_wafv2_web_acl.this.arn
}

output "web_acl_name" {
  value = aws_wafv2_web_acl.this.name
}

output "log_group_name" {
  description = "CloudWatch log group holding this region's WAF request logs."
  value       = aws_cloudwatch_log_group.waf.name
}
