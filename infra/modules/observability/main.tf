variable "name" {
  type = string
}

variable "region" {
  type = string
}

variable "alb_arn_suffix" {
  description = "ALB ARN suffix (app/<name>/<id>) for the LoadBalancer metric dimension. Passed dynamically from the env root — never hardcode, the id changes if the ALB is recreated."
  type        = string
}

variable "alert_email" {
  type = string
}

variable "page_alert_email" {
  description = "Email for high-severity (page) alarms — region/edge down (ALB 503s), OpenSearch RED, broken end-to-end canary journeys. Empty = fall back to alert_email (same recipient) until a pager is wired up."
  type        = string
  default     = ""
}

variable "sqs_queue_name" {
  type = string
}

variable "search_index_queue_name" {
  description = "Name of the search-index-events FIFO queue (videos stream -> Pipe -> this queue)"
  type        = string
}

variable "search_index_dlq_name" {
  description = "Name of the search-index-events dead-letter queue"
  type        = string
}

variable "videos_to_search_pipe_name" {
  description = "Name of the EventBridge Pipe feeding the search index"
  type        = string
}

variable "delete_cleanup_dlq_name" {
  description = "Name of the delete-cleanup dead-letter queue (cascade cleanup)"
  type        = string
}

variable "transcode_completions_eventbridge_dlq_name" {
  description = "Name of the EventBridge-delivery DLQ for the MediaConvert completions rule target)"
  type        = string
}

# Peer region for S3 CRR. Empty in single-region (no replication alarms). When set, this region
# replicates videos to the peer's same-named bucket, and the replication-failure /
# RTC-latency alarms below are created when CRR is enabled. Mirrors regional-data's peer_region.
variable "crr_peer_region" {
  description = "Peer region for S3 CRR; when set, replication alarms are created (empty = none)"
  type        = string
  default     = ""
}

locals {
  crr_enabled = var.crr_peer_region != ""

  # HTTP request-serving services (exclude the queue-driven workers transcode/delete-cleanup, which
  # have no request path). Used for the per-service latency filters/alarms.
  request_serving_services = ["identity", "video-catalog", "upload", "streaming", "social", "search"]
  # Per replicated bucket: the source/destination bucket names + rule id used as the S3 replication
  # metric dimensions (SourceBucket / DestinationBucket / RuleId). Rule ids match regional-data's CRR
  # rules ("<bucket>-to-<peer_region>"). Empty map when CRR is off → no alarms created.
  crr_buckets = local.crr_enabled ? {
    videos = {
      source = "${var.name}-videos-${var.region}"
      dest   = "${var.name}-videos-${var.crr_peer_region}"
      rule   = "videos-to-${var.crr_peer_region}"
    }
  } : {}

  # Global tables (region-free names, mirroring global-data) for DynamoDB Global Table alarms.
  # Replication-latency alarms cover every table (active/active correctness depends on all of them);
  # throttle alarms are scoped to the high-RPS tables to avoid sprawl on rarely-written ones.
  gt_tables          = ["users", "sessions", "videos", "comments", "reactions", "comment_reactions", "video_stats", "view_history", "invite_codes", "verification_tokens", "transcode_jobs"]
  gt_throttle_tables = ["videos", "video_stats", "sessions", "users", "comments"]
}

variable "canary_freshness" {
  description = "Per-tier canary freshness ('is it even running?') alarms: map of tier name => expected emit interval in seconds. Create ONLY for tiers actually scheduled — a suspended tier emits nothing and would alarm forever. E.g. { shallow = 3600 }. Mirrors the chart's per-tier suspend/enable."
  type        = map(number)
  default     = {}
}

variable "opensearch_domain_name" {
  type = string
}

variable "web_acl_name" {
  description = "Name of the regional WAFv2 web ACL on the ALB (the AWS/WAFV2 WebACL dimension), for the blocked-requests alarm."
  type        = string
}

variable "waf_blocked_requests_threshold" {
  description = "Total BlockedRequests (all rules, sum over 5 min) above which the WAF-blocking alarm fires. Set well above the routine scanner/bot-blocking baseline (managed-rule groups block internet background-radiation waves that can reach a few thousand in a 5-min window) so it only fires on a sustained blocking flood, not an ordinary scan. Real-user impact is tracked separately by the rate-limit-rule alarm."
  type        = number
  default     = 3000
}

variable "waf_rate_limit_block_threshold" {
  description = "Blocks by the WAF per-source-IP rate-based rule (sum over 5 min) above which the rate-limit alarm fires. Kept low relative to the aggregate blocked-requests threshold because a rate-limit block, unlike a managed-rule block, plausibly hits real users (shared NAT egress, a too-low limit) — so a modest sustained count is worth investigating."
  type        = number
  default     = 100
}

variable "auth_4xx_threshold" {
  description = "4xx responses on /login + /register (sum over 5 min) above which the auth-4xx alarm fires. Above the routine wrong-password/duplicate-email baseline, so it catches either abuse (credential / invite-code guessing — the usual cause) or a systemic break (auth outage, broken registration/invite validation, bad deploy)."
  type        = number
  default     = 50
}

variable "feed_4xx_threshold" {
  description = "4xx responses on /videos/feed (sum over 5 min) above which the feed-4xx alarm fires. The public feed should almost never 4xx, so a small sustained rate signals a broken client contract."
  type        = number
  default     = 20
}

variable "service_latency_p95_threshold_ms" {
  description = "Per-service p95 request latency (ms, from the log latency_ms field) above which the latency alarm fires. A per-journey signal the aggregate ALB p95 alarm would average out."
  type        = number
  default     = 2000
}

variable "playback_client_errors_threshold" {
  description = "Client-side playback errors (beaconed by VideoPlayer, sum over 5 min) above which the alarm fires. Above the low baseline of one-off client/network hiccups, so it catches a systemic playback break (bad manifest, CORS, codec)."
  type        = number
  default     = 10
}

variable "log_group_prefix" {
  description = "CloudWatch log group for EKS application logs"
  type        = string
}

variable "tags" {
  type    = map(string)
  default = {}
}

# --- SNS Topics (severity-split) ---
#
# Two topics so an outage doesn't page at the same urgency as routine noise (a scanner-driven WAF
# spike, a single thumbnail 404, a replication blip). The bulk of alarms are "investigate" and stay
# on `alerts` (name preserved for backwards-compat + existing subscriptions); a tight, defensible
# "wake me up" set — region/edge effectively down (ALB 503s), OpenSearch cluster RED, and the
# end-to-end canary journeys — routes to `alerts-page` instead. Until a real pager is wired up (P2),
# the page topic defaults to the SAME email as `alert_email`, so this is a pure routing/structure
# change today: no recipient is lost, and swapping in PagerDuty/OpsGenie later is a one-line
# subscription add on the page topic. Individual component alarms can be promoted to `page` as their
# blast radius justifies it (tracked with the composite-alarm work in TASKS.md).

# Low-severity / "investigate" (ticket) topic — the default for the majority of alarms.
resource "aws_sns_topic" "alerts" {
  name = "${var.name}-alerts"
  tags = var.tags
}

resource "aws_sns_topic_subscription" "email" {
  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = var.alert_email
}

# High-severity / "page" topic — outage-class alarms only (see local.page_topic_arn consumers).
resource "aws_sns_topic" "alerts_page" {
  name = "${var.name}-alerts-page"
  tags = var.tags
}

resource "aws_sns_topic_subscription" "page_email" {
  topic_arn = aws_sns_topic.alerts_page.arn
  protocol  = "email"
  endpoint  = var.page_alert_email != "" ? var.page_alert_email : var.alert_email
}

locals {
  # Alarm-action routing. Most alarms use the ticket topic; outage-class alarms use the page topic.
  ticket_topic_arn = aws_sns_topic.alerts.arn
  page_topic_arn   = aws_sns_topic.alerts_page.arn
}

# --- Log Metric Filters ---

resource "aws_cloudwatch_log_metric_filter" "service_5xx" {
  for_each = toset(["identity", "video-catalog", "upload", "streaming", "social", "search", "transcode"])

  name           = "${var.name}-${each.key}-5xx"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"${each.key}\" && $.log_processed.fields.status >= 500 }"

  metric_transformation {
    name      = "${each.key}-5xx-count"
    namespace = "${var.name}/Services"
    value     = "1"
  }
}

resource "aws_cloudwatch_log_metric_filter" "streaming_playback_404" {
  name           = "${var.name}-streaming-playback-404"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"streaming\" && $.log_processed.fields.status = 404 && $.log_processed.span.path = \"/videos/*/stream-url\" }"

  metric_transformation {
    name      = "streaming-playback-404"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

resource "aws_cloudwatch_log_metric_filter" "streaming_thumbnail_404" {
  name           = "${var.name}-streaming-thumbnail-404"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"streaming\" && $.log_processed.fields.status = 404 && $.log_processed.span.path = \"/videos/*/thumbnail-url\" }"

  metric_transformation {
    name      = "streaming-thumbnail-404"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

# Customer-facing upload failures: any 4xx/5xx response on the upload endpoints. Distinct from the
# 5xx-only `upload-broken` alarm — client-caused S3 errors (e.g. /complete with a stale upload_id or
# mismatched parts) now correctly return 4xx (see shared::error::from_aws), so they no longer surface
# as 5xx; this filter catches "users can't finish uploading" regardless of 4xx-vs-5xx classification.
resource "aws_cloudwatch_log_metric_filter" "upload_failing" {
  name           = "${var.name}-upload-failing"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"upload\" && $.log_processed.span.path = \"/uploads/*\" && $.log_processed.fields.status >= 400 }"

  metric_transformation {
    name      = "upload-failures"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

# Auth 4xx (login/register): only 4xx (5xx is covered by login-broken). A routine baseline exists
# (wrong passwords, duplicate emails), so the companion alarm fires only on a surge — a systemic
# break like an auth outage returning 401s, broken registration/invite validation, or a bad deploy.
resource "aws_cloudwatch_log_metric_filter" "auth_4xx" {
  name           = "${var.name}-auth-4xx"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"identity\" && ($.log_processed.span.path = \"/login\" || $.log_processed.span.path = \"/register\") && $.log_processed.fields.status >= 400 && $.log_processed.fields.status < 500 }"

  metric_transformation {
    name      = "auth-4xx-count"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

# Feed 4xx: the public feed should almost never return a 4xx to a normal user, so any sustained rate
# signals a broken client/contract (e.g. a bad deploy changing query params).
resource "aws_cloudwatch_log_metric_filter" "feed_4xx" {
  name           = "${var.name}-feed-4xx"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"video-catalog\" && $.log_processed.span.path = \"/videos/feed\" && $.log_processed.fields.status >= 400 && $.log_processed.fields.status < 500 }"

  metric_transformation {
    name      = "feed-4xx-count"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

# Per-service request latency. The aggregate ALB p95 alarm can hide a single slow journey (one
# service slow while the ALB-wide p95 looks fine), so extract each request-serving service's
# latency_ms (logged by shared::middleware on every response as a number) and alarm per service.
# Excludes /health (fast + high-volume, would skew the percentile). value = the logged latency_ms.
resource "aws_cloudwatch_log_metric_filter" "service_latency" {
  for_each = toset(local.request_serving_services)

  name           = "${var.name}-${each.key}-latency"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"${each.key}\" && $.log_processed.fields.message = \"response\" && $.log_processed.span.path != \"/health\" }"

  metric_transformation {
    name      = "${each.key}-latency-ms"
    namespace = "${var.name}/Services"
    value     = "$.log_processed.fields.latency_ms"
    unit      = "Milliseconds"
  }
}

resource "aws_cloudwatch_log_metric_filter" "transcode_failures" {
  name           = "${var.name}-transcode-failures"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"transcode\" && $.log_processed.fields.message = \"*failed to process job*\" }"

  metric_transformation {
    name      = "transcode-failures"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

resource "aws_cloudwatch_log_metric_filter" "transcode_complete" {
  name           = "${var.name}-transcode-complete"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"transcode\" && $.log_processed.fields.message = \"*transcode complete*\" }"

  metric_transformation {
    name      = "transcode-completions"
    namespace = "${var.name}/Services"
    value     = "1"
  }
}

resource "aws_cloudwatch_log_metric_filter" "search_requests" {
  name           = "${var.name}-search-requests"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"search\" && $.log_processed.span.path = \"/search\" }"

  metric_transformation {
    name      = "search-requests"
    namespace = "${var.name}/Services"
    value     = "1"
  }
}

# Search consumer failed to process a stream message (logged before leaving it for SQS redrive).
# Early-warning signal that complements the DLQ-depth alarm below.
resource "aws_cloudwatch_log_metric_filter" "search_index_failures" {
  name           = "${var.name}-search-index-failures"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"search\" && $.log_processed.fields.message = \"*failed to process stream message*\" }"

  metric_transformation {
    name      = "search-index-failures"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

# Frontend (Next.js) structured server errors. Unlike the Rust services (which log via the shared
# tracing JSON, with status at $.log_processed.fields.status), the frontend's instrumentation.ts onRequestError
# hook emits a flat JSON line with a top-level `status`. Any SSR / route-handler error is a 500, so
# this captures frontend-origin 5xx that the per-service Rust filters cannot see.
resource "aws_cloudwatch_log_metric_filter" "frontend_5xx" {
  name           = "${var.name}-frontend-5xx"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"frontend\" && $.log_processed.status >= 500 }"

  metric_transformation {
    name      = "frontend-5xx-count"
    namespace = "${var.name}/Services"
    value     = "1"
  }
}

# Client-side playback failures beaconed by VideoPlayer to /api/playback-error (hls.js fatal /
# MediaError). Streaming + CloudFront can all return 200s yet the browser still can't play (codec,
# CORS, manifest parse); this is the only signal for that. The beacon logs a flat structured line
# with a stable event marker, so (like frontend_5xx) it lands at $.log_processed.event.
resource "aws_cloudwatch_log_metric_filter" "frontend_playback_errors" {
  name           = "${var.name}-frontend-playback-errors"
  log_group_name = var.log_group_prefix
  pattern        = "{ $.kubernetes.container_name = \"frontend\" && $.log_processed.event = \"playback_error\" }"

  metric_transformation {
    name      = "playback-client-errors"
    namespace = "${var.name}/CustomerExperience"
    value     = "1"
  }
}

# --- Alarms: Infrastructure Health ---

resource "aws_cloudwatch_metric_alarm" "alb_5xx" {
  alarm_name          = "${var.name}-alb-5xx-spike"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "HTTPCode_Target_5XX_Count"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  statistic           = "Sum"
  threshold           = 10
  alarm_description   = "ALB 5xx responses > 10 in 1 minute"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    LoadBalancer = var.alb_arn_suffix
  }

  tags = var.tags
}

# ALB-generated 5xx (HTTPCode_ELB_5XX_Count): the ALB itself returns 5xx — most importantly 503 when
# a target group has NO healthy hosts. This is the signal that the target-5xx alarm and the log-based
# per-service 5xx alarms both MISS (the request never reaches a pod), and it's exactly what fired
# during the node-rotation outage. Threshold kept low so a brief full-region blip still pages.
resource "aws_cloudwatch_metric_alarm" "alb_elb_5xx" {
  alarm_name          = "${var.name}-alb-elb-5xx"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "HTTPCode_ELB_5XX_Count"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  statistic           = "Sum"
  threshold           = 10
  alarm_description   = "ALB-generated 5xx > 10 in 1 minute (e.g. no healthy targets -> 503s)"
  alarm_actions       = [local.page_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    LoadBalancer = var.alb_arn_suffix
  }

  tags = var.tags
}

# Total WAF blocks across ALL rules. This is dominated by the AWS managed rule groups blocking
# internet background-radiation (opportunistic vuln scanners: XSS/LFI/RFI probes, bad IP reputation)
# — expected, benign, and bursty (a single scan wave can block a few thousand in one 5-min window).
# So this is deliberately a HIGH, SUSTAINED alarm: it fires only when blocking stays above the
# threshold across multiple periods (a real blocking flood / volumetric event), not on a lone scan
# burst. The customer-facing question — "are we blocking real users?" — is answered by the
# rate-limit-rule alarm below, NOT this aggregate (managed-rule blocks are almost never real users).
# Dimensions: the web ACL (WebACL + Region); Rule = ALL aggregates every rule's blocks.
resource "aws_cloudwatch_metric_alarm" "waf_blocking_spike" {
  alarm_name          = "${var.name}-waf-blocking-spike"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 3
  datapoints_to_alarm = 2
  metric_name         = "BlockedRequests"
  namespace           = "AWS/WAFV2"
  period              = 300
  statistic           = "Sum"
  threshold           = var.waf_blocked_requests_threshold
  alarm_description   = "WAF blocked > ${var.waf_blocked_requests_threshold} requests/5min in 2 of 3 periods — a sustained blocking flood (not a routine scanner burst)"
  alarm_actions       = [local.ticket_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    WebACL = var.web_acl_name
    Region = var.region
    Rule   = "ALL"
  }

  tags = var.tags
}

# Real-user-impact WAF signal: blocks attributed specifically to the per-source-IP rate-based rule.
# Unlike the managed-rule blocks (scanners), a rate-limit block is a plausible legitimate client —
# e.g. many users behind one NAT/corporate egress IP, or a too-low limit after a traffic change.
# A sustained count here means real users may be getting 403'd at the edge (invisible to ALB/service
# 5xx). The Rule dimension value matches the rate-based rule's name in the WAF module ("rate-limit").
resource "aws_cloudwatch_metric_alarm" "waf_rate_limit_blocking" {
  alarm_name          = "${var.name}-waf-rate-limit-blocking"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "BlockedRequests"
  namespace           = "AWS/WAFV2"
  period              = 300
  statistic           = "Sum"
  threshold           = var.waf_rate_limit_block_threshold
  alarm_description   = "WAF rate-limit rule blocked > ${var.waf_rate_limit_block_threshold} requests in 5 min — real users may be getting rate-limited (NAT egress, too-low limit)"
  alarm_actions       = [local.ticket_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    WebACL = var.web_acl_name
    Region = var.region
    Rule   = "rate-limit"
  }

  tags = var.tags
}

# L7 request-flood / scanner-sweep signal. The rate-based rules only block a source IP that exceeds
# their per-IP limit, so anything staying UNDER that limit passes straight through and is invisible to
# every block-based alarm. Two shapes do this: a distributed flood (many IPs, each modest) and — just
# as commonly — a single-source scanner or scripted sweep whose rate is nowhere near the volumetric
# limit. Either way it only surfaces later as latency/5xx. This watches total inbound RequestCount
# against a CloudWatch anomaly-detection band trained on normal traffic, so an abnormal surge is
# reported up front. Ticket-severity + sustained (2 of 3 periods) to ride out the noise a low-traffic
# baseline produces; genuine customer impact from a flood is separately caught (and paged) by the
# latency/5xx/canary alarms. The band self-trains from history (widens on low/erratic traffic), so
# early on it is a coarse signal that tightens as data accumulates. Note the baseline here is mostly
# Route 53 health checks, which are steady — that makes the band tight, so a modest multiple of
# normal traffic can cross it. Pair this with the WAF request logs to see whether the surge is one
# source or many before treating it as a DDoS.
resource "aws_cloudwatch_metric_alarm" "alb_request_flood" {
  alarm_name          = "${var.name}-alb-request-flood"
  comparison_operator = "GreaterThanUpperThreshold"
  evaluation_periods  = 3
  datapoints_to_alarm = 2
  threshold_metric_id = "band"
  alarm_description   = "ALB request volume is anomalously high — an L7 flood or a scanner sweep, from a single IP or many, staying under the WAF per-IP rate limits. Check the WAF logs to see if it is one source or distributed"
  alarm_actions       = [local.ticket_topic_arn]
  treat_missing_data  = "notBreaching"

  metric_query {
    id          = "reqs"
    return_data = true
    metric {
      metric_name = "RequestCount"
      namespace   = "AWS/ApplicationELB"
      period      = 300
      stat        = "Sum"
      dimensions  = { LoadBalancer = var.alb_arn_suffix }
    }
  }
  metric_query {
    id          = "band"
    expression  = "ANOMALY_DETECTION_BAND(reqs, 3)"
    label       = "RequestCount (expected band)"
    return_data = true
  }

  tags = var.tags
}

resource "aws_cloudwatch_metric_alarm" "sqs_age" {
  alarm_name          = "${var.name}-transcode-pipeline-stuck"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateAgeOfOldestMessage"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Maximum"
  threshold           = 300
  alarm_description   = "Transcode job stuck > 5 minutes"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = var.sqs_queue_name
  }

  tags = var.tags
}

resource "aws_cloudwatch_metric_alarm" "sqs_backlog" {
  alarm_name          = "${var.name}-transcode-backlog"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 5
  metric_name         = "ApproximateNumberOfMessagesVisible"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Average"
  threshold           = 50
  alarm_description   = "Transcode backlog > 50 messages for 5 minutes"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = var.sqs_queue_name
  }

  tags = var.tags
}

resource "aws_cloudwatch_metric_alarm" "opensearch_red" {
  alarm_name          = "${var.name}-opensearch-cluster-red"
  comparison_operator = "GreaterThanOrEqualToThreshold"
  evaluation_periods  = 1
  metric_name         = "ClusterStatus.red"
  namespace           = "AWS/ES"
  period              = 60
  statistic           = "Maximum"
  threshold           = 1
  alarm_description   = "OpenSearch cluster status is RED"
  alarm_actions       = [local.page_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    DomainName = var.opensearch_domain_name
    ClientId   = data.aws_caller_identity.current.account_id
  }

  tags = var.tags
}

resource "aws_cloudwatch_metric_alarm" "opensearch_disk" {
  alarm_name          = "${var.name}-opensearch-disk-low"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 1
  metric_name         = "FreeStorageSpace"
  namespace           = "AWS/ES"
  period              = 300
  statistic           = "Minimum"
  threshold           = 3000
  alarm_description   = "OpenSearch free storage < 3GB"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    DomainName = var.opensearch_domain_name
    ClientId   = data.aws_caller_identity.current.account_id
  }

  tags = var.tags
}

# JVM memory pressure on the (single-node) OpenSearch domain. This is a known, documented failure
# mode for this platform: sustained high heap pressure drives long GC pauses that stall queries,
# time out the search client, and flap the read-path canary — all while OpenSearch's own query
# latency looks fine. cluster-red + disk alarms don't see it. Maximum > 80% sustained over 15 min
# is the standard early-warning threshold (well below the ~92% level at which OpenSearch starts
# blocking writes), giving headroom to react (rightsize the node) before it turns into an outage.
resource "aws_cloudwatch_metric_alarm" "opensearch_jvm_pressure" {
  alarm_name          = "${var.name}-opensearch-jvm-pressure"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 3
  metric_name         = "JVMMemoryPressure"
  namespace           = "AWS/ES"
  period              = 300
  statistic           = "Maximum"
  threshold           = 80
  alarm_description   = "OpenSearch JVM memory pressure > 80% for 15 min — GC stalls degrade/timeout search before cluster status turns RED"
  alarm_actions       = [local.ticket_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    DomainName = var.opensearch_domain_name
    ClientId   = data.aws_caller_identity.current.account_id
  }

  tags = var.tags
}

# Sustained high CPU on the single-node domain — the other saturation axis behind the same
# read-path degradation (a burstable instance running out of CPU credits stalls queries just like
# heap pressure does). Ticket-severity early warning to rightsize before it becomes customer-facing.
resource "aws_cloudwatch_metric_alarm" "opensearch_cpu" {
  alarm_name          = "${var.name}-opensearch-cpu-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 3
  metric_name         = "CPUUtilization"
  namespace           = "AWS/ES"
  period              = 300
  statistic           = "Maximum"
  threshold           = 80
  alarm_description   = "OpenSearch CPU > 80% for 15 min — search queries degrade/time out under sustained CPU saturation"
  alarm_actions       = [local.ticket_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    DomainName = var.opensearch_domain_name
    ClientId   = data.aws_caller_identity.current.account_id
  }

  tags = var.tags
}

# --- Search index sync pipeline (videos stream -> Pipe -> SQS FIFO -> search consumer) ---

# Authoritative "search index has lost sync" signal: a message failed processing maxReceiveCount
# times and landed in the DLQ. Any message here means OpenSearch is drifting from the videos table.
resource "aws_cloudwatch_metric_alarm" "search_index_dlq" {
  alarm_name          = "${var.name}-search-index-dlq-not-empty"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateNumberOfMessagesVisible"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Maximum"
  threshold           = 0
  alarm_description   = "Search-index events landed in the DLQ — index is drifting from the videos table"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = var.search_index_dlq_name
  }

  tags = var.tags
}

# --- Cascade delete-cleanup (videos stream -> Pipe -> SQS FIFO -> delete-cleanup worker) ---

# The one operator-impacting signal for the cleanup pipeline: a cleanup message failed
# maxReceiveCount times and dead-lettered, so a deleted video's dependent rows/objects are NOT
# being reclaimed (orphaned storage + stale stats, and — once multi-region — orphans that replicate).
# Cleanup is async/fire-and-forget, so this is the authoritative "cleanup is silently broken" alarm.
resource "aws_cloudwatch_metric_alarm" "delete_cleanup_dlq" {
  alarm_name          = "${var.name}-delete-cleanup-dlq-not-empty"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateNumberOfMessagesVisible"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Maximum"
  threshold           = 0
  alarm_description   = "Delete-cleanup messages landed in the DLQ — a deleted video's data is not being reclaimed"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = var.delete_cleanup_dlq_name
  }

  tags = var.tags
}

# Poison transcode jobs that exhausted SQS retries land in the transcode-jobs DLQ. Any message
# means an upload's transcode failed permanently. The stuck-`processing` reconciler also catches the
# symptom (the job marks the video `processing` first); this alarms on the cause. Name derived to
# match regional-data ("${var.name}-transcode-jobs-dlq"). Mirrors the search/cleanup DLQ alarms.
resource "aws_cloudwatch_metric_alarm" "transcode_dlq" {
  alarm_name          = "${var.name}-transcode-jobs-dlq-not-empty"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateNumberOfMessagesVisible"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Maximum"
  threshold           = 0
  alarm_description   = "Transcode jobs landed in the DLQ — an upload's transcode failed permanently"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = "${var.name}-transcode-jobs-dlq"
  }

  tags = var.tags
}

# Consumer is down or falling behind: events are piling up and aging. The index goes stale.
resource "aws_cloudwatch_metric_alarm" "search_index_queue_stuck" {
  alarm_name          = "${var.name}-search-index-pipeline-stuck"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateAgeOfOldestMessage"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Maximum"
  threshold           = 300
  alarm_description   = "Search-index event unprocessed > 5 minutes (consumer down or behind)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = var.search_index_queue_name
  }

  tags = var.tags
}

# Source-side blind spot: if the Pipe fails to read the stream or write to SQS, events never reach
# the queue, so the queue alarms stay quiet while the index silently drifts. (Metric: AWS/Pipes.)
resource "aws_cloudwatch_metric_alarm" "videos_to_search_pipe_failing" {
  alarm_name          = "${var.name}-search-index-pipe-failing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ExecutionFailed"
  namespace           = "AWS/Pipes"
  period              = 300
  statistic           = "Sum"
  threshold           = 0
  alarm_description   = "EventBridge Pipe feeding the search index is failing (stream read or SQS write)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    PipeName = var.videos_to_search_pipe_name
  }

  tags = var.tags
}

# Cleanup-side counterpart to the search-pipe alarm. If the videos->cleanup Pipe fails to read
# the stream or write to the cleanup queue, soft-delete events never reach the worker, so a deleted
# video's dependent data is never reclaimed — and no DLQ catches it (nothing was ever enqueued).
# Name derived to match regional-data ("${var.name}-videos-to-cleanup").
resource "aws_cloudwatch_metric_alarm" "videos_to_cleanup_pipe_failing" {
  alarm_name          = "${var.name}-delete-cleanup-pipe-failing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ExecutionFailed"
  namespace           = "AWS/Pipes"
  period              = 300
  statistic           = "Sum"
  threshold           = 0
  alarm_description   = "EventBridge Pipe feeding delete-cleanup is failing — deleted videos' data won't be reclaimed"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    PipeName = "${var.name}-videos-to-cleanup"
  }

  tags = var.tags
}

# Early warning: consumer processing errors (before they exhaust retries into the DLQ).
resource "aws_cloudwatch_metric_alarm" "search_index_sync_failing" {
  alarm_name          = "${var.name}-search-index-sync-failing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "search-index-failures"
  namespace           = "${var.name}/CustomerExperience"
  period              = 300
  statistic           = "Sum"
  threshold           = 10
  alarm_description   = "Search consumer failing to process stream messages (> 10 in 5 min)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

resource "aws_cloudwatch_metric_alarm" "alb_latency" {
  alarm_name          = "${var.name}-high-latency"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 5
  metric_name         = "TargetResponseTime"
  namespace           = "AWS/ApplicationELB"
  period              = 60
  extended_statistic  = "p95"
  threshold           = 3
  alarm_description   = "ALB p95 latency > 3s for 5 minutes"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    LoadBalancer = var.alb_arn_suffix
  }

  tags = var.tags
}

# Telemetry heartbeat — "are my log-based alarms even alive?". ~half the customer-experience alarms
# are log-metric-filters that use treat_missing_data=notBreaching, so if the log pipeline breaks
# (Fluent Bit down, IAM/addon failure, or the structured-log JSON shape drifts) those alarms go
# silent and simply never fire — a blind spot that hides real customer pain. This alarm inverts the
# usual pattern: it watches raw log ingestion into the application log group and treats ABSENCE as
# the failure (treat_missing_data=breaching), so "no logs arriving" pages instead of passing quietly.
# It keys on IncomingLogEvents (shape-independent — it counts events, not parsed fields), which has a
# constant nonzero baseline here (ALB health checks + the hourly canary + real traffic), so a drop to
# zero for ~15 min genuinely means the pipeline is broken. Routed to the page topic: if this fires,
# the log-based half of the alarm suite can no longer be trusted.
resource "aws_cloudwatch_metric_alarm" "log_pipeline_heartbeat" {
  alarm_name          = "${var.name}-log-pipeline-silent"
  comparison_operator = "LessThanOrEqualToThreshold"
  evaluation_periods  = 3
  datapoints_to_alarm = 3
  metric_name         = "IncomingLogEvents"
  namespace           = "AWS/Logs"
  period              = 300
  statistic           = "Sum"
  threshold           = 0
  alarm_description   = "No application logs ingested for ~15 min — the log pipeline is down and the log-based (customer-experience) alarms are blind"
  alarm_actions       = [local.page_topic_arn]
  treat_missing_data  = "breaching"

  dimensions = {
    LogGroupName = var.log_group_prefix
  }

  tags = var.tags
}

# --- Alarms: Customer Experience (from log metric filters) ---

resource "aws_cloudwatch_metric_alarm" "streaming_playback_failing" {
  alarm_name          = "${var.name}-video-playback-failing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "streaming-playback-404"
  namespace           = "${var.name}/CustomerExperience"
  period              = 60
  statistic           = "Sum"
  threshold           = 5
  alarm_description   = "Video playback failures > 5 in 1 minute"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

resource "aws_cloudwatch_metric_alarm" "thumbnails_missing" {
  alarm_name          = "${var.name}-thumbnails-missing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "streaming-thumbnail-404"
  namespace           = "${var.name}/CustomerExperience"
  period              = 60
  statistic           = "Sum"
  threshold           = 10
  alarm_description   = "Thumbnail 404s > 10 in 1 minute"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

resource "aws_cloudwatch_metric_alarm" "transcode_job_failures" {
  alarm_name          = "${var.name}-transcode-failures"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "transcode-failures"
  namespace           = "${var.name}/CustomerExperience"
  period              = 300
  statistic           = "Sum"
  threshold           = 1
  alarm_description   = "Transcode job failed"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

resource "aws_cloudwatch_metric_alarm" "upload_broken" {
  alarm_name          = "${var.name}-upload-broken"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "upload-5xx-count"
  namespace           = "${var.name}/Services"
  period              = 300
  statistic           = "Sum"
  threshold           = 2
  alarm_description   = "Upload service 5xx > 2 in 5 minutes"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# Customer-experience view of uploads: any 4xx/5xx on the upload endpoints (via the upload_failing
# filter). `upload-broken` above stays the pure 5xx infra-fault signal; this fires on the user-facing
# symptom — a wave of failed uploads — including the client-caused 4xx (stale/expired upload session,
# unassemblable parts) that `upload-broken` no longer sees now that those return 4xx instead of 500.
resource "aws_cloudwatch_metric_alarm" "upload_failing" {
  alarm_name          = "${var.name}-upload-failing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "upload-failures"
  namespace           = "${var.name}/CustomerExperience"
  period              = 300
  statistic           = "Sum"
  threshold           = 3
  alarm_description   = "Upload failures (4xx/5xx on /uploads/*) > 3 in 5 minutes"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# Auth 4xx surge (login/register) — see the auth_4xx filter. Threshold sits above the routine
# wrong-password/duplicate-email baseline so it fires on a systemic break or on abuse, not normal
# user error. Ordered by observed likelihood: the common cause is credential / invite-code guessing
# from a scanner or bot, which is abuse rather than breakage — the platform working as designed while
# something hammers it. The signature distinguishes them: mostly 401s on /login is credential
# stuffing, mostly 400s on /register is invite-code guessing, and a broad mix across both (especially
# alongside a `-broken` 5xx alarm or a fresh deploy) points at a genuine regression instead. Check
# the WAF request logs for the source before assuming a bad deploy.
resource "aws_cloudwatch_metric_alarm" "auth_4xx_spike" {
  alarm_name          = "${var.name}-auth-4xx-spike"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "auth-4xx-count"
  namespace           = "${var.name}/CustomerExperience"
  period              = 300
  statistic           = "Sum"
  threshold           = var.auth_4xx_threshold
  alarm_description   = "Auth 4xx (login/register) > ${var.auth_4xx_threshold} in 5 min — usually credential/invite-code guessing; also an auth outage, broken registration/invite validation, or a bad deploy. Check the WAF logs for a source IP"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# Feed 4xx — see the feed_4xx filter. The public feed should essentially never 4xx.
resource "aws_cloudwatch_metric_alarm" "feed_4xx_spike" {
  alarm_name          = "${var.name}-feed-4xx-spike"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "feed-4xx-count"
  namespace           = "${var.name}/CustomerExperience"
  period              = 300
  statistic           = "Sum"
  threshold           = var.feed_4xx_threshold
  alarm_description   = "Feed 4xx (/videos/feed) > ${var.feed_4xx_threshold} in 5 min — broken client contract on the public feed"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# Per-service p95 latency (from the service_latency filters). Fires when one journey is slow even if
# the ALB-wide p95 stays healthy. Sustained 5 min to avoid flapping on transient spikes.
resource "aws_cloudwatch_metric_alarm" "service_latency_high" {
  for_each = toset(local.request_serving_services)

  alarm_name          = "${var.name}-${each.key}-latency-high"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 5
  metric_name         = "${each.key}-latency-ms"
  namespace           = "${var.name}/Services"
  period              = 60
  extended_statistic  = "p95"
  threshold           = var.service_latency_p95_threshold_ms
  alarm_description   = "${each.key} p95 request latency > ${var.service_latency_p95_threshold_ms}ms for 5 minutes"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

resource "aws_cloudwatch_metric_alarm" "login_broken" {
  alarm_name          = "${var.name}-login-broken"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "identity-5xx-count"
  namespace           = "${var.name}/Services"
  period              = 300
  statistic           = "Sum"
  threshold           = 3
  alarm_description   = "Identity service 5xx > 3 in 5 minutes (login/register broken)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

resource "aws_cloudwatch_metric_alarm" "search_broken" {
  alarm_name          = "${var.name}-search-broken"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "search-5xx-count"
  namespace           = "${var.name}/Services"
  period              = 60
  statistic           = "Sum"
  threshold           = 3
  alarm_description   = "Search service 5xx > 3 in 1 minute"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

resource "aws_cloudwatch_metric_alarm" "social_broken" {
  alarm_name          = "${var.name}-social-broken"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "social-5xx-count"
  namespace           = "${var.name}/Services"
  period              = 60
  statistic           = "Sum"
  threshold           = 5
  alarm_description   = "Social service 5xx > 5 in 1 minute (comments/likes broken)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# video-catalog is the core feed/watch/CRUD service; it had a 5xx metric filter but no alarm.
resource "aws_cloudwatch_metric_alarm" "video_catalog_broken" {
  alarm_name          = "${var.name}-video-catalog-broken"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "video-catalog-5xx-count"
  namespace           = "${var.name}/Services"
  period              = 300
  statistic           = "Sum"
  threshold           = 3
  alarm_description   = "video-catalog 5xx > 3 in 5 minutes (feed/watch/CRUD broken)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# streaming had only 404 alarms (playback/thumbnail), not a 5xx alarm.
resource "aws_cloudwatch_metric_alarm" "streaming_broken" {
  alarm_name          = "${var.name}-streaming-broken"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "streaming-5xx-count"
  namespace           = "${var.name}/Services"
  period              = 300
  statistic           = "Sum"
  threshold           = 3
  alarm_description   = "streaming 5xx > 3 in 5 minutes (playback URL issuance broken)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# Frontend server-side errors (SSR / route handlers), via the frontend_5xx log filter above.
resource "aws_cloudwatch_metric_alarm" "frontend_broken" {
  alarm_name          = "${var.name}-frontend-broken"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "frontend-5xx-count"
  namespace           = "${var.name}/Services"
  period              = 300
  statistic           = "Sum"
  threshold           = 3
  alarm_description   = "Frontend server 5xx > 3 in 5 minutes (SSR / route-handler errors)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# Client-side playback failures (from the frontend_playback_errors filter). The only signal for
# "video won't play in the browser" when the server returned 200s everywhere. Threshold sits above
# the low baseline of one-off client/network hiccups so it fires on a systemic break.
resource "aws_cloudwatch_metric_alarm" "playback_client_errors" {
  alarm_name          = "${var.name}-playback-client-errors"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "playback-client-errors"
  namespace           = "${var.name}/CustomerExperience"
  period              = 300
  statistic           = "Sum"
  threshold           = var.playback_client_errors_threshold
  alarm_description   = "Client-side playback errors > ${var.playback_client_errors_threshold} in 5 min — viewers can't play video even though the server returned 200s"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"
  tags                = var.tags
}

# --- Canary ---
# The canary emits CanarySuccess = 1 (all steps passed) or 0 (any step failed) to the fixed
# "Rewind/Canary" namespace (see services/canary/src/metrics.rs), dimensioned by Tier + Region. A
# value < 1 means the live, user-facing journey is broken end-to-end. treat_missing_data is
# notBreaching so the alarm stays OK while the CronJobs are suspended / between runs (a real
# failure posts a 0 that trips Minimum < 1 within the period).
resource "aws_cloudwatch_metric_alarm" "canary_deep_failing" {
  alarm_name          = "${var.name}-canary-deep-failing"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 1
  metric_name         = "CanarySuccess"
  namespace           = "Rewind/Canary"
  period              = 3600
  statistic           = "Minimum"
  threshold           = 1
  alarm_description   = "Deep canary failed — the multi-actor journey (auth/social/stream/delete+cascade) is broken"
  alarm_actions       = [local.page_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    Tier   = "deep"
    Region = var.region
  }

  tags = var.tags
}

resource "aws_cloudwatch_metric_alarm" "canary_shallow_failing" {
  alarm_name          = "${var.name}-canary-shallow-failing"
  comparison_operator = "LessThanThreshold"
  # M-of-N: page only when 2 of the last 3 hourly runs failed, not on a single run. The shallow tier
  # runs hourly (one datapoint per 3600s period, so Minimum = that run's 1/0), and its dependencies
  # include a cold (hourly-idle) connection to OpenSearch whose handshake can intermittently stall and
  # fail one run in isolation. Requiring 2 of 3 absorbs that single-run flake while still paging
  # promptly on a sustained read-path outage (≤ ~3h). The companion canary-*-stale alarm still catches
  # a canary that stops emitting entirely.
  evaluation_periods  = 3
  datapoints_to_alarm = 2
  metric_name         = "CanarySuccess"
  namespace           = "Rewind/Canary"
  period              = 3600
  statistic           = "Minimum"
  threshold           = 1
  alarm_description   = "Shallow canary failed 2 of the last 3 runs — health/feed/search read path is broken"
  alarm_actions       = [local.page_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    Tier   = "shallow"
    Region = var.region
  }

  tags = var.tags
}

# Dedicated region-routing alarm. The shallow canary's `region-routing` step
# resolves a latency-routed public host + this region's region-pinned host and asserts they hit the
# same ALB — i.e. Route 53 latency routing kept this region's traffic in-region. It posts a per-step
# StepSuccess (Tier=shallow, Region, Step=region-routing): 0 means this region was mis-routed to the
# OTHER region's ALB. A failure also trips canary-shallow-failing (region-routing is one of its
# steps), but this alarm pinpoints *routing* as the cause rather than a generic read-path failure.
# treat_missing_data notBreaching mirrors the *-failing alarms (stays OK between runs).
resource "aws_cloudwatch_metric_alarm" "canary_region_routing_failing" {
  alarm_name          = "${var.name}-canary-region-routing-failing"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 1
  metric_name         = "StepSuccess"
  namespace           = "Rewind/Canary"
  period              = 3600
  statistic           = "Minimum"
  threshold           = 1
  alarm_description   = "Shallow canary region-routing step failed in ${var.region} — Route 53 latency routing sent this region's traffic to the other region's ALB"
  alarm_actions       = [local.page_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    Tier   = "shallow"
    Region = var.region
    Step   = "region-routing"
  }

  tags = var.tags
}
# Those use treat_missing_data=notBreaching, so a canary that silently STOPS emitting (suspended by
# mistake, broken image, RBAC/SA failure) never trips them — absence of failure isn't success. This
# alarm fires on ABSENCE: treat_missing_data=breaching means a window with no datapoint = the
# scheduled canary isn't running. It keys on SampleCount (not Minimum) so a *failed* run — which still
# posts CanarySuccess=0, i.e. SampleCount=1 — does NOT trip this (that's the *-failing alarm's job);
# only true silence does. Scoped to enabled/scheduled tiers via var.canary_freshness (period = that
# tier's cadence); evaluation_periods=2 tolerates one skipped run before paging.
resource "aws_cloudwatch_metric_alarm" "canary_stale" {
  for_each = var.canary_freshness

  alarm_name          = "${var.name}-canary-${each.key}-stale"
  comparison_operator = "LessThanThreshold"
  evaluation_periods  = 2
  datapoints_to_alarm = 2
  metric_name         = "CanarySuccess"
  namespace           = "Rewind/Canary"
  period              = each.value
  statistic           = "SampleCount"
  threshold           = 1
  alarm_description   = "Canary '${each.key}' in ${var.region} has not reported in ~2 intervals — the scheduled canary may not be running"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "breaching"

  dimensions = {
    Tier   = each.key
    Region = var.region
  }

  tags = var.tags
}

# --- Transcode resilience ---

# A video sits in `status=processing` until a MediaConvert completion event publishes it. If that
# event is lost (exhausted EventBridge delivery, never emitted) or the region dies mid-transcode,
# nothing flips it out of `processing` and the upload silently never plays — no DLQ catches it,
# because nothing *failed*. The per-region reconcile CronJob scans the videos table each tick and
# emits Rewind/Transcode StuckTranscodes (count of stranded jobs, dimensioned by Region), so any
# value >= 1 means at least one transcode is stranded. Emitted every run (incl. 0) so the alarm
# clears when jobs unstick; treat_missing_data notBreaching covers the gap between CronJob ticks.
resource "aws_cloudwatch_metric_alarm" "transcode_stuck" {
  alarm_name          = "${var.name}-transcode-stuck-processing"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "StuckTranscodes"
  namespace           = "Rewind/Transcode"
  period              = 3600
  statistic           = "Maximum"
  threshold           = 0
  alarm_description   = "A transcode is stranded in `processing` (lost completion event or region failure)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    Region = var.region
  }

  tags = var.tags
}

# --- Cascade-cleanup resilience ---

# A soft-delete is supposed to reclaim a video's dependent rows + S3 objects via the videos stream ->
# Pipe -> cleanup worker. If the Pipe never enqueues the event (nothing reaches the queue, so the DLQ
# stays empty) or cleanup only partially completes and is not redriven, the dependent data is
# silently orphaned — neither the delete-cleanup DLQ nor the Pipe-failure alarm catches it. The
# per-region reconcile CronJob scans the videos table for `deleted` tombstones, probes whether their
# dependent data still exists, and emits Rewind/Deletion UnreclaimedDeletions (count of orphaned
# tombstones, dimensioned by Region), so any value >= 1 means at least one delete was never fully
# reclaimed. Emitted every run (incl. 0) so the alarm clears once reclaimed; treat_missing_data
# notBreaching covers the gap between CronJob ticks. Mirrors the stuck-transcode alarm above.
resource "aws_cloudwatch_metric_alarm" "delete_cleanup_unreclaimed" {
  alarm_name          = "${var.name}-delete-cleanup-unreclaimed"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "UnreclaimedDeletions"
  namespace           = "Rewind/Deletion"
  period              = 3600
  statistic           = "Maximum"
  threshold           = 0
  alarm_description   = "A deleted video's dependent data was never reclaimed (failed cleanup Pipe or partial cleanup)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    Region = var.region
  }

  tags = var.tags
}
# or dropped) land here. Any message means a MediaConvert result was lost in transit — the video
# would be stranded in `processing`. Complements the stuck-transcode alarm (which catches the
# symptom) by pinpointing the delivery-loss cause. Mirrors the search/cleanup DLQ alarms.
resource "aws_cloudwatch_metric_alarm" "transcode_completions_eventbridge_dlq" {
  alarm_name          = "${var.name}-transcode-completions-eventbridge-dlq-not-empty"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateNumberOfMessagesVisible"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Maximum"
  threshold           = 0
  alarm_description   = "MediaConvert completion events failed EventBridge delivery — a transcode result was lost (video stuck in processing)"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = var.transcode_completions_eventbridge_dlq_name
  }

  tags = var.tags
}

# Completion events that the publish consumer failed to process maxReceiveCount times land in the
# transcode-completions SQS DLQ. Any message = a MediaConvert result that never got applied, so the
# video is stranded in `processing` and never plays. This is the SQS-consumer-side counterpart to the
# EventBridge-delivery DLQ alarm above (which catches loss in transit); together they cover both ways
# a completion can be lost. The stuck-transcode reconciler catches the resulting symptom too, but
# only after its 60-min threshold — this fires immediately on the silent breakage. Name derived to
# match regional-data ("${var.name}-transcode-completions-dlq"). Mirrors the other DLQ alarms.
resource "aws_cloudwatch_metric_alarm" "transcode_completions_dlq" {
  alarm_name          = "${var.name}-transcode-completions-dlq-not-empty"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ApproximateNumberOfMessagesVisible"
  namespace           = "AWS/SQS"
  period              = 60
  statistic           = "Maximum"
  threshold           = 0
  alarm_description   = "Transcode completion events landed in the SQS DLQ — a MediaConvert result was never applied (video stuck in processing)"
  alarm_actions       = [local.ticket_topic_arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    QueueName = "${var.name}-transcode-completions-dlq"
  }

  tags = var.tags
}

# --- S3 Cross-Region Replication (videos + thumbnails) — only when CRR is enabled ---
# CRR is async/fire-and-forget, so a broken replica is silent (the other region just drifts). These
# alarms are this region's OUTBOUND replication to the peer. Per AWS guidance, replication-metric
# alarms use treat_missing_data = "ignore" (a metric isn't emitted when there's nothing to replicate;
# ignoring missing data holds the last state instead of flapping to INSUFFICIENT_DATA).

# Any failed replication operation = an object that did not reach the peer region.
resource "aws_cloudwatch_metric_alarm" "s3_replication_failed" {
  for_each = local.crr_buckets

  alarm_name          = "${var.name}-${each.key}-replication-failed"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "OperationsFailedReplication"
  namespace           = "AWS/S3"
  period              = 300
  statistic           = "Sum"
  threshold           = 0
  alarm_description   = "S3 CRR failed to replicate ${each.key} objects to ${var.crr_peer_region} — that region is missing content"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "ignore"

  dimensions = {
    SourceBucket      = each.value.source
    DestinationBucket = each.value.dest
    RuleId            = each.value.rule
  }

  tags = var.tags
}

# Replication falling behind the 15-min RTC SLA (ReplicationLatency is in seconds).
resource "aws_cloudwatch_metric_alarm" "s3_replication_latency" {
  for_each = local.crr_buckets

  alarm_name          = "${var.name}-${each.key}-replication-latency"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ReplicationLatency"
  namespace           = "AWS/S3"
  period              = 300
  statistic           = "Maximum"
  threshold           = 900
  alarm_description   = "S3 CRR of ${each.key} to ${var.crr_peer_region} is breaching the 15-min RTC SLA"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "ignore"

  dimensions = {
    SourceBucket      = each.value.source
    DestinationBucket = each.value.dest
    RuleId            = each.value.rule
  }

  tags = var.tags
}

# --- DynamoDB Global Table replication (only when a peer region exists) ---
# The DynamoDB counterpart to the S3 CRR alarms above: Global Tables replicate every table to the
# peer region asynchronously, so replication lag is silent — the other region just serves stale
# data. ReplicationLatency is per (TableName, ReceivingRegion), in ms. treat_missing_data = "ignore"
# mirrors the S3 CRR alarms (no datapoint emitted when there is nothing to replicate).
resource "aws_cloudwatch_metric_alarm" "ddb_replication_latency" {
  for_each = local.crr_enabled ? toset(local.gt_tables) : toset([])

  alarm_name          = "${var.name}-${each.key}-ddb-replication-latency"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  metric_name         = "ReplicationLatency"
  namespace           = "AWS/DynamoDB"
  period              = 300
  statistic           = "Maximum"
  threshold           = 180000 # 3 minutes, in milliseconds
  alarm_description   = "DynamoDB Global Table '${each.key}' replication to ${var.crr_peer_region} is lagging > 3 min"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "ignore"

  dimensions = {
    TableName       = "${var.name}-${each.key}"
    ReceivingRegion = var.crr_peer_region
  }

  tags = var.tags
}

# Throttling on the high-RPS tables (sum of read + write throttle events). With on-demand capacity
# this is rare, but a hot partition or a replica catching up can throttle, dropping requests.
resource "aws_cloudwatch_metric_alarm" "ddb_throttled" {
  for_each = toset(local.gt_throttle_tables)

  alarm_name          = "${var.name}-${each.key}-ddb-throttled"
  comparison_operator = "GreaterThanThreshold"
  evaluation_periods  = 1
  threshold           = 0
  alarm_description   = "DynamoDB table '${each.key}' is throttling requests"
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  metric_query {
    id          = "throttles"
    expression  = "reads + writes"
    label       = "Total throttle events"
    return_data = true
  }
  metric_query {
    id = "reads"
    metric {
      metric_name = "ReadThrottleEvents"
      namespace   = "AWS/DynamoDB"
      period      = 300
      stat        = "Sum"
      dimensions  = { TableName = "${var.name}-${each.key}" }
    }
  }
  metric_query {
    id = "writes"
    metric {
      metric_name = "WriteThrottleEvents"
      namespace   = "AWS/DynamoDB"
      period      = 300
      stat        = "Sum"
      dimensions  = { TableName = "${var.name}-${each.key}" }
    }
  }

  tags = var.tags
}

# --- Data source ---

data "aws_caller_identity" "current" {}

# --- Outputs ---

output "sns_topic_arn" {
  value = aws_sns_topic.alerts.arn
}
