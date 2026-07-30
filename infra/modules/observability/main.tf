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

variable "log_group_prefix" {
  description = "CloudWatch log group for EKS application logs"
  type        = string
}

variable "tags" {
  type    = map(string)
  default = {}
}

# --- SNS Topic (single email subscriber) ---

resource "aws_sns_topic" "alerts" {
  name = "${var.name}-alerts"
  tags = var.tags
}

resource "aws_sns_topic_subscription" "email" {
  topic_arn = aws_sns_topic.alerts.arn
  protocol  = "email"
  endpoint  = var.alert_email
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
  alarm_actions       = [aws_sns_topic.alerts.arn]
  treat_missing_data  = "notBreaching"

  dimensions = {
    LoadBalancer = var.alb_arn_suffix
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
  alarm_actions       = [aws_sns_topic.alerts.arn]
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
  alarm_actions       = [aws_sns_topic.alerts.arn]
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
  alarm_actions       = [aws_sns_topic.alerts.arn]
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
  alarm_actions       = [aws_sns_topic.alerts.arn]
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
