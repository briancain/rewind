variable "name" {
  description = "Name prefix"
  type        = string
}

variable "oidc_provider_arn" {
  description = "EKS OIDC provider ARN"
  type        = string
}

variable "oidc_provider_url" {
  description = "EKS OIDC provider URL (without https://)"
  type        = string
}

variable "namespace" {
  description = "Kubernetes namespace for service accounts"
  type        = string
  default     = "rewind"
}

variable "dynamodb_table_arns" {
  description = "Map of DynamoDB table ARNs"
  type        = map(string)
}

variable "s3_bucket_arns" {
  description = "Map of S3 bucket ARNs"
  type        = map(string)
}

variable "sqs_queue_arn" {
  description = "SQS transcode queue ARN"
  type        = string
}

variable "search_index_queue_arn" {
  description = "SQS search-index-events queue ARN (videos stream -> Pipe -> this queue)"
  type        = string
}

variable "mediaconvert_role_arn" {
  description = "ARN of the MediaConvert service role that transcode passes to CreateJob"
  type        = string
}

variable "transcode_completions_queue_arn" {
  description = "SQS queue ARN carrying MediaConvert completion events"
  type        = string
}

variable "delete_cleanup_queue_arn" {
  description = "SQS delete-cleanup FIFO queue ARN (videos stream -> Pipe -> this queue)"
  type        = string
}

variable "tags" {
  description = "Additional tags"
  type        = map(string)
  default     = {}
}

locals {
  oidc_issuer = replace(var.oidc_provider_url, "https://", "")

  # Per-service IAM policy definitions
  service_policies = {
    identity = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect = "Allow"
        Action = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:DeleteItem", "dynamodb:Query", "dynamodb:Scan", "dynamodb:UpdateItem"]
        Resource = [
          var.dynamodb_table_arns["users"],
          "${var.dynamodb_table_arns["users"]}/index/*",
          var.dynamodb_table_arns["sessions"],
          "${var.dynamodb_table_arns["sessions"]}/index/*",
          var.dynamodb_table_arns["invite_codes"],
          var.dynamodb_table_arns["verification_tokens"],
        ]
      }]
    })

    video-catalog = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect = "Allow"
        Action = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:DeleteItem", "dynamodb:Query", "dynamodb:Scan", "dynamodb:UpdateItem"]
        Resource = [
          var.dynamodb_table_arns["videos"],
          "${var.dynamodb_table_arns["videos"]}/index/*",
          var.dynamodb_table_arns["sessions"],
        ]
      }]
    })

    upload = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["s3:PutObject", "s3:CreateMultipartUpload", "s3:CompleteMultipartUpload", "s3:AbortMultipartUpload", "s3:UploadPart"]
          Resource = "${var.s3_bucket_arns["raw"]}/*"
        },
        {
          Effect   = "Allow"
          Action   = ["sqs:SendMessage"]
          Resource = var.sqs_queue_arn
        },
        {
          Effect = "Allow"
          Action = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:UpdateItem"]
          Resource = [
            var.dynamodb_table_arns["videos"],
            var.dynamodb_table_arns["sessions"],
          ]
        },
      ]
    })

    transcode = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"]
          Resource = var.sqs_queue_arn
        },
        {
          Effect   = "Allow"
          Action   = ["s3:GetObject"]
          Resource = "${var.s3_bucket_arns["raw"]}/*"
        },
        {
          Effect   = "Allow"
          Action   = ["s3:PutObject"]
          Resource = ["${var.s3_bucket_arns["videos"]}/*"]
        },
        {
          Effect   = "Allow"
          Action   = ["dynamodb:GetItem", "dynamodb:UpdateItem", "dynamodb:PutItem"]
          Resource = [var.dynamodb_table_arns["videos"], var.dynamodb_table_arns["transcode_jobs"]]
        },
        {
          # Submit transcoding jobs to MediaConvert. Probe reads input metadata (duration) so the
          # generated thumbnail can be placed at ~25% of the video instead of the black frame 0.
          Effect   = "Allow"
          Action   = ["mediaconvert:CreateJob", "mediaconvert:GetJob", "mediaconvert:Probe"]
          Resource = "*"
        },
        {
          # Pass the MediaConvert service role to CreateJob (scoped so it can only be passed to MC).
          Effect   = "Allow"
          Action   = ["iam:PassRole"]
          Resource = var.mediaconvert_role_arn
          Condition = {
            StringEquals = { "iam:PassedToService" = "mediaconvert.amazonaws.com" }
          }
        },
        {
          # Drain the MediaConvert completion-event queue.
          Effect   = "Allow"
          Action   = ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"]
          Resource = var.transcode_completions_queue_arn
        },
      ]
    })

    streaming = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["s3:GetObject"]
          Resource = ["${var.s3_bucket_arns["videos"]}/*"]
        },
        {
          Effect   = "Allow"
          Action   = ["dynamodb:GetItem"]
          Resource = [var.dynamodb_table_arns["videos"], var.dynamodb_table_arns["sessions"]]
        },
      ]
    })

    social = jsonencode({
      Version = "2012-10-17"
      Statement = [{
        Effect = "Allow"
        Action = ["dynamodb:GetItem", "dynamodb:PutItem", "dynamodb:DeleteItem", "dynamodb:Query", "dynamodb:UpdateItem"]
        Resource = [
          var.dynamodb_table_arns["comments"],
          var.dynamodb_table_arns["reactions"],
          var.dynamodb_table_arns["video_stats"],
          var.dynamodb_table_arns["comment_reactions"],
          var.dynamodb_table_arns["view_history"],
          "${var.dynamodb_table_arns["view_history"]}/index/*",
          var.dynamodb_table_arns["sessions"],
        ]
      }]
    })

    search = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["es:ESHttp*"]
          Resource = "*"
        },
        {
          # Consume the videos-stream events delivered by the EventBridge Pipe.
          Effect   = "Allow"
          Action   = ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"]
          Resource = var.search_index_queue_arn
        },
        {
          # Read the videos table (source of truth) for the /reindex backfill.
          Effect = "Allow"
          Action = ["dynamodb:Scan", "dynamodb:GetItem"]
          Resource = [
            var.dynamodb_table_arns["videos"],
            "${var.dynamodb_table_arns["videos"]}/index/*",
          ]
        },
      ]
    })

    # Cascade delete-cleanup worker: reclaims a deleted video's dependent data.
    delete-cleanup = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          # Query each table by video_id (base or via the video-id-index GSI) and batch-delete.
          Effect = "Allow"
          Action = [
            "dynamodb:Query",
            "dynamodb:BatchWriteItem",
            "dynamodb:DeleteItem",
          ]
          Resource = [
            var.dynamodb_table_arns["comments"],
            var.dynamodb_table_arns["reactions"],
            var.dynamodb_table_arns["comment_reactions"],
            var.dynamodb_table_arns["video_stats"],
            var.dynamodb_table_arns["view_history"],
            "${var.dynamodb_table_arns["view_history"]}/index/*",
            var.dynamodb_table_arns["transcode_jobs"],
            "${var.dynamodb_table_arns["transcode_jobs"]}/index/*",
          ]
        },
        {
          # Delete the video's S3 objects (hls/mp4/thumbnails in videos, raw/ in raw). ListBucket on
          # the bucket, DeleteObject on its contents.
          Effect = "Allow"
          Action = ["s3:ListBucket"]
          Resource = [
            var.s3_bucket_arns["videos"],
            var.s3_bucket_arns["raw"],
          ]
        },
        {
          Effect = "Allow"
          Action = ["s3:DeleteObject"]
          Resource = [
            "${var.s3_bucket_arns["videos"]}/*",
            "${var.s3_bucket_arns["raw"]}/*",
          ]
        },
        {
          # Drain the delete-cleanup queue.
          Effect   = "Allow"
          Action   = ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"]
          Resource = var.delete_cleanup_queue_arn
        },
        {
          # Invalidate the deleted video's paths on the content CDN (cdn.<domain>) so "deleted" means
          # gone at the edge, not just at origin. Resource is "*": the CloudFront distribution lives
          # in the dedicated infra/cdn stack and its id is random (not name-derivable), and the env
          # cannot read that stack's state without creating an env<->cdn dependency cycle — so the
          # arn can't be referenced here. CreateInvalidation is low-risk (a cache-purge hint), matching
          # the "*"-scoped grants already used elsewhere (canary/transcode-reconcile PutMetricData).
          Effect   = "Allow"
          Action   = ["cloudfront:CreateInvalidation"]
          Resource = "*"
        },
      ]
    })

    # Cloud integration canary. Blackbox tester that runs as a CronJob. It seeds a
    # little data out-of-band (an invite + a per-run video, and removes its ephemeral user), then
    # verifies the cascade by READING the dependent tables/buckets — it never hand-deletes that
    # data (the product's DELETE /videos cascade does). Hence read-only on the dependent stores.
    canary = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          # Seed a one-time invite for the ephemeral auth user (and tidy unused ones).
          Effect   = "Allow"
          Action   = ["dynamodb:PutItem", "dynamodb:DeleteItem"]
          Resource = var.dynamodb_table_arns["invite_codes"]
        },
        {
          # Remove the ephemeral auth user at the end of a deep run.
          Effect   = "Allow"
          Action   = ["dynamodb:DeleteItem"]
          Resource = var.dynamodb_table_arns["users"]
        },
        {
          # Seed the per-run unlisted video (published + manifest_url) the deep run exercises.
          Effect   = "Allow"
          Action   = ["dynamodb:PutItem"]
          Resource = var.dynamodb_table_arns["videos"]
        },
        {
          # Read-only cascade verification: after DELETE /videos, confirm every dependent row is
          # gone. Query (base + GSI) on the social/transcode tables; GetItem on the stats counter.
          Effect = "Allow"
          Action = ["dynamodb:Query", "dynamodb:GetItem"]
          Resource = [
            var.dynamodb_table_arns["comments"],
            var.dynamodb_table_arns["reactions"],
            var.dynamodb_table_arns["comment_reactions"],
            var.dynamodb_table_arns["video_stats"],
            var.dynamodb_table_arns["view_history"],
            "${var.dynamodb_table_arns["view_history"]}/index/*",
            var.dynamodb_table_arns["transcode_jobs"],
            "${var.dynamodb_table_arns["transcode_jobs"]}/index/*",
          ]
        },
        {
          # Read-only cascade verification of the video's S3 objects (hls/mp4/thumbnails + raw).
          Effect   = "Allow"
          Action   = ["s3:ListBucket"]
          Resource = [var.s3_bucket_arns["videos"], var.s3_bucket_arns["raw"]]
        },
        {
          # Emit the Rewind/Canary per-step success + latency metrics.
          Effect   = "Allow"
          Action   = ["cloudwatch:PutMetricData"]
          Resource = "*"
        },
      ]
    })

    # Stuck-`processing` transcode reconciler. Runs as a per-region
    # CronJob — the transcode image invoked as `transcode reconcile`. DETECT + ALARM only: scans the
    # videos (Global) table for stranded jobs and emits a CloudWatch metric. Deliberately minimal +
    # read-only — NO UpdateItem and NO queue-send (automated re-drive is deferred), so this
    # role cannot mutate a video or re-enqueue a job. Least-privilege over reusing the transcode role.
    transcode-reconcile = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          Effect   = "Allow"
          Action   = ["dynamodb:Scan"]
          Resource = var.dynamodb_table_arns["videos"]
        },
        {
          Effect   = "Allow"
          Action   = ["cloudwatch:PutMetricData"]
          Resource = "*"
        },
      ]
    })

    # Cascade-cleanup reconciler. Runs as a per-region CronJob — the delete-cleanup image invoked as
    # `delete-cleanup reconcile`. DETECT + ALARM only: scans the videos (Global) table for `deleted`
    # tombstones, then PROBES each dependent store (read-only) for leftovers and emits a CloudWatch
    # metric. Deliberately minimal + read-only — only Scan/Query/GetItem + ListBucket (no DeleteItem,
    # no BatchWriteItem, no DeleteObject), so it cannot reclaim anything (automated re-cleanup is
    # deferred). Least-privilege over reusing the delete-cleanup role.
    delete-cleanup-reconcile = jsonencode({
      Version = "2012-10-17"
      Statement = [
        {
          # Find deleted tombstones.
          Effect   = "Allow"
          Action   = ["dynamodb:Scan"]
          Resource = var.dynamodb_table_arns["videos"]
        },
        {
          # Probe each dependent table by video_id (base or via the video-id-index GSI) for leftovers.
          Effect = "Allow"
          Action = ["dynamodb:Query", "dynamodb:GetItem"]
          Resource = [
            var.dynamodb_table_arns["comments"],
            var.dynamodb_table_arns["reactions"],
            var.dynamodb_table_arns["comment_reactions"],
            var.dynamodb_table_arns["video_stats"],
            var.dynamodb_table_arns["view_history"],
            "${var.dynamodb_table_arns["view_history"]}/index/*",
            var.dynamodb_table_arns["transcode_jobs"],
            "${var.dynamodb_table_arns["transcode_jobs"]}/index/*",
          ]
        },
        {
          # Probe the video's S3 prefixes (hls/mp4/thumbnails in videos, raw/ in raw) for leftovers.
          Effect = "Allow"
          Action = ["s3:ListBucket"]
          Resource = [
            var.s3_bucket_arns["videos"],
            var.s3_bucket_arns["raw"],
          ]
        },
        {
          Effect   = "Allow"
          Action   = ["cloudwatch:PutMetricData"]
          Resource = "*"
        },
      ]
    })
  }
}

resource "aws_iam_role" "service" {
  for_each = local.service_policies

  name = "${var.name}-${each.key}"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Principal = {
        Federated = var.oidc_provider_arn
      }
      Action = "sts:AssumeRoleWithWebIdentity"
      Condition = {
        StringEquals = {
          "${local.oidc_issuer}:aud" = "sts.amazonaws.com"
          "${local.oidc_issuer}:sub" = "system:serviceaccount:${var.namespace}:${each.key}"
        }
      }
    }]
  })

  tags = var.tags
}

resource "aws_iam_role_policy" "service" {
  for_each = local.service_policies

  name   = "${each.key}-policy"
  role   = aws_iam_role.service[each.key].id
  policy = each.value
}

output "role_arns" {
  value = { for k, v in aws_iam_role.service : k => v.arn }
}
