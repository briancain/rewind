# infra/observability — global, multi-region CloudWatch dashboard.
#
# CloudWatch dashboards are a GLOBAL (account-wide) resource, so a single dashboard is owned here,
# once, rather than per-region in the observability module (which would collide on the shared name —
# the dashboard-name collision fix). Regional resources (alarms, SNS, log config) remain in the
# per-region modules/observability module. This stack is data-driven by `var.regions`: it reads each
# region's ALB suffix from that region's remote state and derives the region-free resource names by
# convention, rendering one stacked band of widgets per region for a single active/active pane.

terraform {
  required_version = ">= 1.5"

  backend "s3" {
    bucket         = "rewind-terraform-state"
    key            = "observability/terraform.tfstate"
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

# Dashboards are global; the provider region only selects the API endpoint for the PutDashboard call.
provider "aws" {
  region  = "us-west-2"
  profile = var.aws_profile

  default_tags {
    tags = {
      Project   = "rewind"
      ManagedBy = "terraform"
      Component = "observability"
    }
  }
}

data "aws_caller_identity" "current" {}

# Per-region env state — supplies each region's ALB ARN suffix (the CloudWatch LoadBalancer
# dimension). Keyed by region so widgets can look it up as
# data.terraform_remote_state.region[<region>].outputs.alb_arn_suffix.
data "terraform_remote_state" "region" {
  for_each = toset(var.regions)

  backend = "s3"
  config = {
    bucket  = var.state_bucket
    key     = "dev/${each.key}/terraform.tfstate"
    region  = var.state_region
    profile = var.aws_profile
  }
}

locals {
  account_id = data.aws_caller_identity.current.account_id

  # Region-free resource names (regional resources use region-free names by convention; each widget's
  # `region` property scopes the metric to that region). Mirrors modules/regional-data + the env.
  sqs_queue_name          = "${var.name}-transcode-jobs"
  search_index_queue_name = "${var.name}-search-index-events.fifo"
  search_index_dlq_name   = "${var.name}-search-index-events-dlq.fifo"
  videos_to_search_pipe   = "${var.name}-videos-to-search"
  cleanup_queue_name      = "${var.name}-delete-cleanup-events.fifo"
  cleanup_dlq_name        = "${var.name}-delete-cleanup-dlq.fifo"
  videos_to_cleanup_pipe  = "${var.name}-videos-to-cleanup"
  opensearch_domain_name  = "${var.name}-search"
  completions_eb_dlq_name = "${var.name}-transcode-completions-eventbridge-dlq"

  services = ["identity", "video-catalog", "upload", "streaming", "social", "search", "transcode", "frontend"]

  # Each region occupies a vertical band: a height-2 header text widget, then the 36-row metric grid
  # (original single-region layout offset by +2). Band height = 38; region idx i starts at i*38.
  band_height = 38

  widgets = flatten([
    for idx, r in var.regions : [
      {
        type   = "text"
        x      = 0
        y      = idx * local.band_height
        width  = 24
        height = 2
        properties = {
          markdown = "## ${upper(r)} — region ${idx + 1} of ${length(var.regions)} · active/active. All metrics below are scoped to **${r}**."
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = idx * local.band_height + 2
        width  = 12
        height = 6
        properties = {
          title  = "Request Count by Service"
          region = r
          metrics = [
            for svc in local.services :
            ["${var.name}/Services", "${svc}-5xx-count", { label = "${svc} 5xx", stat = "Sum", period = 60 }]
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = idx * local.band_height + 2
        width  = 12
        height = 6
        properties = {
          title  = "ALB Latency (p50/p95/p99)"
          region = r
          metrics = [
            ["AWS/ApplicationELB", "TargetResponseTime", "LoadBalancer", data.terraform_remote_state.region[r].outputs.alb_arn_suffix, { stat = "p50", label = "p50" }],
            ["AWS/ApplicationELB", "TargetResponseTime", "LoadBalancer", data.terraform_remote_state.region[r].outputs.alb_arn_suffix, { stat = "p95", label = "p95" }],
            ["AWS/ApplicationELB", "TargetResponseTime", "LoadBalancer", data.terraform_remote_state.region[r].outputs.alb_arn_suffix, { stat = "p99", label = "p99" }],
          ]
          view   = "timeSeries"
          period = 60
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = idx * local.band_height + 8
        width  = 8
        height = 6
        properties = {
          title  = "Transcode Pipeline"
          region = r
          metrics = [
            ["${var.name}/Services", "transcode-completions", { stat = "Sum", period = 300, label = "Completed/5min" }],
            ["${var.name}/CustomerExperience", "transcode-failures", { stat = "Sum", period = 300, label = "Failures/5min" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 8
        y      = idx * local.band_height + 8
        width  = 8
        height = 6
        properties = {
          title  = "SQS Queue"
          region = r
          metrics = [
            ["AWS/SQS", "ApproximateNumberOfMessagesVisible", "QueueName", local.sqs_queue_name, { stat = "Average", period = 60, label = "Visible" }],
            ["AWS/SQS", "ApproximateAgeOfOldestMessage", "QueueName", local.sqs_queue_name, { stat = "Maximum", period = 60, label = "Age (s)" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 16
        y      = idx * local.band_height + 8
        width  = 8
        height = 6
        properties = {
          title  = "Customer Experience"
          region = r
          metrics = [
            ["${var.name}/CustomerExperience", "streaming-playback-404", { stat = "Sum", period = 60, label = "Playback 404s" }],
            ["${var.name}/CustomerExperience", "streaming-thumbnail-404", { stat = "Sum", period = 60, label = "Thumbnail 404s" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = idx * local.band_height + 14
        width  = 12
        height = 6
        properties = {
          title  = "OpenSearch"
          region = r
          metrics = [
            ["AWS/ES", "FreeStorageSpace", "DomainName", local.opensearch_domain_name, "ClientId", local.account_id, { stat = "Minimum", period = 300, label = "Free Storage (MB)" }],
            ["AWS/ES", "SearchRate", "DomainName", local.opensearch_domain_name, "ClientId", local.account_id, { stat = "Average", period = 60, label = "Search Rate" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = idx * local.band_height + 14
        width  = 12
        height = 6
        properties = {
          title  = "Search Queries"
          region = r
          metrics = [
            ["${var.name}/Services", "search-requests", { stat = "Sum", period = 60, label = "Queries/min" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = idx * local.band_height + 20
        width  = 12
        height = 6
        properties = {
          title  = "Search Index Sync Pipeline"
          region = r
          metrics = [
            ["AWS/SQS", "ApproximateNumberOfMessagesVisible", "QueueName", local.search_index_queue_name, { stat = "Average", period = 60, label = "Queue backlog" }],
            ["AWS/SQS", "ApproximateAgeOfOldestMessage", "QueueName", local.search_index_queue_name, { stat = "Maximum", period = 60, label = "Oldest age (s)" }],
            ["AWS/SQS", "ApproximateNumberOfMessagesVisible", "QueueName", local.search_index_dlq_name, { stat = "Maximum", period = 60, label = "DLQ depth" }],
            ["${var.name}/CustomerExperience", "search-index-failures", { stat = "Sum", period = 60, label = "Consumer errors" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = idx * local.band_height + 20
        width  = 12
        height = 6
        properties = {
          title  = "Search Index Pipe (EventBridge)"
          region = r
          metrics = [
            ["AWS/Pipes", "Invocations", "PipeName", local.videos_to_search_pipe, { stat = "Sum", period = 60, label = "Invocations" }],
            ["AWS/Pipes", "ExecutionFailed", "PipeName", local.videos_to_search_pipe, { stat = "Sum", period = 60, label = "Failed" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = idx * local.band_height + 26
        width  = 12
        height = 6
        properties = {
          title  = "Cascade Deletion"
          region = r
          metrics = [
            ["AWS/SQS", "ApproximateNumberOfMessagesVisible", "QueueName", local.cleanup_queue_name, { stat = "Average", period = 60, label = "Cleanup backlog" }],
            ["AWS/SQS", "ApproximateAgeOfOldestMessage", "QueueName", local.cleanup_queue_name, { stat = "Maximum", period = 60, label = "Oldest age (s)" }],
            ["AWS/SQS", "ApproximateNumberOfMessagesVisible", "QueueName", local.cleanup_dlq_name, { stat = "Maximum", period = 60, label = "DLQ depth" }],
            ["AWS/Pipes", "ExecutionFailed", "PipeName", local.videos_to_cleanup_pipe, { stat = "Sum", period = 60, label = "Pipe failures" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = idx * local.band_height + 26
        width  = 12
        height = 6
        properties = {
          title  = "Canary"
          region = r
          metrics = [
            ["Rewind/Canary", "CanarySuccess", "Tier", "deep", "Region", r, { stat = "Minimum", period = 3600, label = "deep success (1=pass)" }],
            ["Rewind/Canary", "CanarySuccess", "Tier", "shallow", "Region", r, { stat = "Minimum", period = 3600, label = "shallow success (1=pass)" }],
          ]
          view  = "timeSeries"
          yAxis = { left = { min = 0, max = 1 } }
        }
      },
      {
        type   = "metric"
        x      = 0
        y      = idx * local.band_height + 32
        width  = 12
        height = 6
        properties = {
          title  = "Transcode Resilience"
          region = r
          metrics = [
            ["Rewind/Transcode", "StuckTranscodes", "Region", r, { stat = "Maximum", period = 3600, label = "Stuck processing jobs" }],
            ["AWS/SQS", "ApproximateNumberOfMessagesVisible", "QueueName", local.completions_eb_dlq_name, { stat = "Maximum", period = 60, label = "Completions delivery DLQ depth" }],
          ]
          view = "timeSeries"
        }
      },
      {
        type   = "metric"
        x      = 12
        y      = idx * local.band_height + 32
        width  = 12
        height = 6
        properties = {
          title  = "Canary — Region Routing"
          region = r
          metrics = [
            ["Rewind/Canary", "StepSuccess", "Tier", "shallow", "Region", r, "Step", "region-routing", { stat = "Minimum", period = 3600, label = "in-region routing (1=correct)" }],
          ]
          view  = "timeSeries"
          yAxis = { left = { min = 0, max = 1 } }
        }
      },
    ]
  ])
}

resource "aws_cloudwatch_dashboard" "overview" {
  dashboard_name = var.name
  dashboard_body = jsonencode({ widgets = local.widgets })
}
