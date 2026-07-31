# regional-data — the per-region data-plane resources: S3 buckets, SQS queues, OpenSearch, the
# MediaConvert role + completion path, and the two EventBridge Pipes off the videos stream.
#
# Split from global-data (the DynamoDB tables) so this whole module can be stood up independently in
# each region. The videos *stream* ARN is passed IN: in the home region it's the
# table's stream; in a secondary region it's that region's Global Table replica stream (looked up by
# the caller). Regional resource names are region-free (rewind-dev-*) — they live in separate regional
# namespaces so they don't collide. IAM roles (global namespace) use the region-qualified `iam_name`.

variable "name" {
  description = "Region-free name prefix for regional resources (queues, OpenSearch, EB), e.g. rewind-dev"
  type        = string
}

variable "iam_name" {
  description = "Region-qualified prefix for IAM roles (account-global namespace), e.g. rewind-dev-us-west-2"
  type        = string
}

variable "region" {
  description = "AWS region"
  type        = string
}

variable "vpc_id" {
  description = "VPC ID for OpenSearch"
  type        = string
}

variable "private_subnet_ids" {
  description = "Private subnet IDs for OpenSearch"
  type        = list(string)
}

variable "search_role_arn" {
  description = "IAM role ARN for OpenSearch master user (search service IRSA role)"
  type        = string
}

variable "videos_stream_arn" {
  description = "DynamoDB stream ARN of the videos table (this region's replica) feeding the Pipes"
  type        = string
}

variable "tags" {
  description = "Additional tags"
  type        = map(string)
  default     = {}
}

# Peer region for S3 Cross-Region Replication of the videos bucket. Empty in
# single-region (no CRR). At expansion each region sets this to the OTHER region (e.g. us-west-2's
# peer is us-east-2 and vice-versa) to get bidirectional CRR. The peer's bucket is the same-named,
# region-suffixed bucket (`${name}-videos-${peer_region}`), so the destination ARN is
# derived by convention — no cross-stack dependency.
variable "peer_region" {
  description = "Peer region for bidirectional S3 CRR (empty = no replication)"
  type        = string
  default     = ""
}

locals {
  crr_enabled = var.peer_region != ""
}

# --- S3 Buckets (region-suffixed names — S3 is a global namespace) ---

resource "aws_s3_bucket" "raw" {
  bucket = "${var.name}-raw-${var.region}"
  tags   = var.tags
}

resource "aws_s3_bucket" "videos" {
  bucket = "${var.name}-videos-${var.region}"
  tags   = var.tags
}

# Neither bucket is ever served directly: `raw` is written via presigned PUT and read only by
# transcode, and `videos` is fronted by CloudFront with an Origin Access Control. Public access is
# blocked explicitly rather than relying on the account-level S3 default, so the posture is visible
# in code and survives an account whose defaults were relaxed.
resource "aws_s3_bucket_public_access_block" "raw" {
  bucket = aws_s3_bucket.raw.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

resource "aws_s3_bucket_public_access_block" "videos" {
  bucket = aws_s3_bucket.videos.id

  block_public_acls       = true
  block_public_policy     = true
  ignore_public_acls      = true
  restrict_public_buckets = true
}

# Versioning on videos (P0-4): S3 Cross-Region Replication requires versioning on BOTH
# source and destination, so it must be enabled before CRR is configured at multi-region expansion.
# Raw stays unversioned — it is regional-only (transcode runs where the upload landed) and CRR'd raw
# is unnecessary; it also auto-expires at 30 days.
resource "aws_s3_bucket_versioning" "videos" {
  bucket = aws_s3_bucket.videos.id
  versioning_configuration {
    status = "Enabled"
  }
}

# --- S3 Cross-Region Replication (videos) — bidirectional at expansion ---
# Gated on peer_region: empty in single-region (none of the below is created). When set, this region
# replicates its videos to the peer region's same-named bucket with Replication Time
# Control (15-min SLA + metrics). Raw is NOT replicated (regional-only; auto-expires at 30d). Delete
# markers are NOT replicated — per-region deletes are handled by the cascade-cleanup worker, so
# replicating deletes would risk cross-region races.

resource "aws_iam_role" "s3_replication" {
  count = local.crr_enabled ? 1 : 0
  name  = "${var.iam_name}-s3-replication"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "s3.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = var.tags
}

resource "aws_iam_role_policy" "s3_replication" {
  count = local.crr_enabled ? 1 : 0
  name  = "s3-replication"
  role  = aws_iam_role.s3_replication[0].id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        # Read the replication config + list the source buckets.
        Effect   = "Allow"
        Action   = ["s3:GetReplicationConfiguration", "s3:ListBucket"]
        Resource = [aws_s3_bucket.videos.arn]
      },
      {
        # Read source object versions (+ ACL/tagging) for replication.
        Effect = "Allow"
        Action = [
          "s3:GetObjectVersionForReplication",
          "s3:GetObjectVersionAcl",
          "s3:GetObjectVersionTagging",
        ]
        Resource = ["${aws_s3_bucket.videos.arn}/*"]
      },
      {
        # Write the replicas into the peer region's same-named buckets (derived by convention).
        Effect   = "Allow"
        Action   = ["s3:ReplicateObject", "s3:ReplicateDelete", "s3:ReplicateTags"]
        Resource = ["arn:aws:s3:::${var.name}-videos-${var.peer_region}/*"]
      },
    ]
  })
}

resource "aws_s3_bucket_replication_configuration" "videos" {
  count      = local.crr_enabled ? 1 : 0
  role       = aws_iam_role.s3_replication[0].arn
  bucket     = aws_s3_bucket.videos.id
  depends_on = [aws_s3_bucket_versioning.videos]

  rule {
    id     = "videos-to-${var.peer_region}"
    status = "Enabled"
    filter {}

    # Per-region cascade-cleanup handles deletes; do not replicate delete markers.
    delete_marker_replication {
      status = "Disabled"
    }

    destination {
      bucket        = "arn:aws:s3:::${var.name}-videos-${var.peer_region}"
      storage_class = "STANDARD"

      replication_time {
        status = "Enabled"
        time {
          minutes = 15
        }
      }
      metrics {
        status = "Enabled"
        event_threshold {
          minutes = 15
        }
      }
    }
  }
}

resource "aws_s3_bucket_lifecycle_configuration" "raw" {
  bucket = aws_s3_bucket.raw.id

  rule {
    id     = "delete-after-30-days"
    status = "Enabled"

    filter {}

    expiration {
      days = 30
    }
  }

  # Reclaim abandoned multipart uploads. A failed /uploads/complete leaves an *incomplete* multipart
  # upload behind; its parts are not S3 objects, so the 30-day object expiration above never removes
  # them and they accrue storage cost indefinitely (a stranded 725 MB upload from a live-demo failure
  # is what surfaced this). Abort incomplete uploads 7 days after initiation — well beyond any
  # legitimate large-upload window.
  rule {
    id     = "abort-incomplete-multipart-uploads"
    status = "Enabled"

    filter {}

    abort_incomplete_multipart_upload {
      days_after_initiation = 7
    }
  }
}

resource "aws_s3_bucket_cors_configuration" "raw" {
  bucket = aws_s3_bucket.raw.id

  cors_rule {
    allowed_headers = ["*"]
    allowed_methods = ["GET", "PUT", "POST", "HEAD"]
    allowed_origins = ["*"]
    expose_headers  = ["ETag"]
  }
}

# CORS on the videos bucket so hls.js (cross-origin XHR) can fetch manifests + segments.
resource "aws_s3_bucket_cors_configuration" "videos" {
  bucket = aws_s3_bucket.videos.id

  cors_rule {
    allowed_headers = ["*"]
    allowed_methods = ["GET", "HEAD"]
    allowed_origins = ["*"]
    expose_headers  = ["ETag"]
    max_age_seconds = 3000
  }
}

# --- SQS Queues ---

resource "aws_sqs_queue" "transcode_dlq" {
  name                      = "${var.name}-transcode-jobs-dlq"
  message_retention_seconds = 1209600 # 14 days
  tags                      = var.tags
}

resource "aws_sqs_queue" "transcode" {
  name                       = "${var.name}-transcode-jobs"
  visibility_timeout_seconds = 300
  message_retention_seconds  = 86400

  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.transcode_dlq.arn
    maxReceiveCount     = 5
  })

  tags = var.tags
}

# MediaConvert completion-event path: MediaConvert -> EventBridge -> this SQS queue.
resource "aws_sqs_queue" "transcode_completions_dlq" {
  name                      = "${var.name}-transcode-completions-dlq"
  message_retention_seconds = 1209600 # 14 days
  tags                      = var.tags
}

resource "aws_sqs_queue" "transcode_completions" {
  name                       = "${var.name}-transcode-completions"
  visibility_timeout_seconds = 60
  message_retention_seconds  = 86400

  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.transcode_completions_dlq.arn
    maxReceiveCount     = 5
  })

  tags = var.tags
}

# Search index sync pipeline queues (FIFO, ordered per video_id).
resource "aws_sqs_queue" "search_index_dlq" {
  name                      = "${var.name}-search-index-events-dlq.fifo"
  fifo_queue                = true
  message_retention_seconds = 1209600 # 14 days
  tags                      = var.tags
}

resource "aws_sqs_queue" "search_index_events" {
  name                        = "${var.name}-search-index-events.fifo"
  fifo_queue                  = true
  content_based_deduplication = true
  visibility_timeout_seconds  = 60
  message_retention_seconds   = 86400

  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.search_index_dlq.arn
    maxReceiveCount     = 5
  })

  tags = var.tags
}

# Cascade delete-cleanup queues (FIFO).
resource "aws_sqs_queue" "delete_cleanup_dlq" {
  name                      = "${var.name}-delete-cleanup-dlq.fifo"
  fifo_queue                = true
  message_retention_seconds = 1209600 # 14 days
  tags                      = var.tags
}

resource "aws_sqs_queue" "delete_cleanup_events" {
  name                        = "${var.name}-delete-cleanup-events.fifo"
  fifo_queue                  = true
  content_based_deduplication = true
  visibility_timeout_seconds  = 120
  message_retention_seconds   = 86400

  redrive_policy = jsonencode({
    deadLetterTargetArn = aws_sqs_queue.delete_cleanup_dlq.arn
    maxReceiveCount     = 5
  })

  tags = var.tags
}

# --- EventBridge Pipes off the videos stream ---

# Search-index Pipe: videos stream -> FIFO queue (no filter; the consumer derives the action for
# every event type). MessageGroupId = video_id for per-video ordering.
resource "aws_iam_role" "videos_to_search_pipe" {
  name = "${var.iam_name}-videos-to-search-pipe"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "pipes.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = var.tags
}

resource "aws_iam_role_policy" "videos_to_search_pipe" {
  name = "videos-to-search-pipe"
  role = aws_iam_role.videos_to_search_pipe.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "dynamodb:DescribeStream",
          "dynamodb:GetRecords",
          "dynamodb:GetShardIterator",
          "dynamodb:ListStreams",
        ]
        Resource = var.videos_stream_arn
      },
      {
        Effect   = "Allow"
        Action   = ["sqs:SendMessage"]
        Resource = aws_sqs_queue.search_index_events.arn
      },
    ]
  })
}

resource "aws_pipes_pipe" "videos_to_search" {
  name     = "${var.name}-videos-to-search"
  role_arn = aws_iam_role.videos_to_search_pipe.arn
  source   = var.videos_stream_arn
  target   = aws_sqs_queue.search_index_events.arn

  source_parameters {
    dynamodb_stream_parameters {
      starting_position = "LATEST"
      batch_size        = 10
    }
  }

  target_parameters {
    sqs_queue_parameters {
      message_group_id = "$.dynamodb.Keys.video_id.S"
    }
  }

  tags = var.tags

  depends_on = [aws_iam_role_policy.videos_to_search_pipe]
}

# Cascade-cleanup Pipe: videos stream -> cleanup FIFO queue, FILTERED to soft-deletes
# (NewImage.status == "deleted"). The literal mirrors VideoStatus::Deleted (shared::video).
resource "aws_iam_role" "videos_to_cleanup_pipe" {
  name = "${var.iam_name}-videos-to-cleanup-pipe"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "pipes.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = var.tags
}

resource "aws_iam_role_policy" "videos_to_cleanup_pipe" {
  name = "videos-to-cleanup-pipe"
  role = aws_iam_role.videos_to_cleanup_pipe.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "dynamodb:DescribeStream",
          "dynamodb:GetRecords",
          "dynamodb:GetShardIterator",
          "dynamodb:ListStreams",
        ]
        Resource = var.videos_stream_arn
      },
      {
        Effect   = "Allow"
        Action   = ["sqs:SendMessage"]
        Resource = aws_sqs_queue.delete_cleanup_events.arn
      },
    ]
  })
}

resource "aws_pipes_pipe" "videos_to_cleanup" {
  name     = "${var.name}-videos-to-cleanup"
  role_arn = aws_iam_role.videos_to_cleanup_pipe.arn
  source   = var.videos_stream_arn
  target   = aws_sqs_queue.delete_cleanup_events.arn

  source_parameters {
    dynamodb_stream_parameters {
      starting_position = "LATEST"
      batch_size        = 10
    }

    filter_criteria {
      filter {
        pattern = jsonencode({
          dynamodb = { NewImage = { status = { S = ["deleted"] } } }
        })
      }
    }
  }

  target_parameters {
    sqs_queue_parameters {
      message_group_id = "$.dynamodb.Keys.video_id.S"
    }
  }

  tags = var.tags

  depends_on = [aws_iam_role_policy.videos_to_cleanup_pipe]
}

# --- MediaConvert service role + completion event routing ---

resource "aws_iam_role" "mediaconvert" {
  name = "${var.iam_name}-mediaconvert"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "mediaconvert.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })

  tags = var.tags
}

resource "aws_iam_role_policy" "mediaconvert_s3" {
  name = "mediaconvert-s3"
  role = aws_iam_role.mediaconvert.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect   = "Allow"
        Action   = ["s3:GetObject"]
        Resource = "${aws_s3_bucket.raw.arn}/*"
      },
      {
        # Writes HLS, MP4, and thumbnail outputs (all under the videos bucket).
        Effect   = "Allow"
        Action   = ["s3:GetObject", "s3:PutObject"]
        Resource = "${aws_s3_bucket.videos.arn}/*"
      },
    ]
  })
}

resource "aws_cloudwatch_event_rule" "mediaconvert_jobs" {
  name        = "${var.name}-mediaconvert-job-state"
  description = "Route MediaConvert COMPLETE/ERROR job events to the transcode completions queue"

  event_pattern = jsonencode({
    source        = ["aws.mediaconvert"]
    "detail-type" = ["MediaConvert Job State Change"]
    detail        = { status = ["COMPLETE", "ERROR"] }
  })

  tags = var.tags
}

resource "aws_cloudwatch_event_target" "mediaconvert_to_sqs" {
  rule = aws_cloudwatch_event_rule.mediaconvert_jobs.name
  arn  = aws_sqs_queue.transcode_completions.arn

  # EventBridge retries delivery to the completions queue (default 24h / 185 attempts). If delivery
  # still can't be made (or is dropped immediately, e.g. a permissions error), the event would be
  # silently lost — which strands the video in `processing`. Capture those events in a delivery DLQ
  # (distinct from the SQS *redrive* DLQ, which only catches consumer-side processing failures) and
  # alarm on it. We cap the retry window at 1h so a stuck event dead-letters (and alarms) promptly
  # rather than retrying silently for a full day.
  retry_policy {
    maximum_event_age_in_seconds = 3600
    maximum_retry_attempts       = 185
  }

  dead_letter_config {
    arn = aws_sqs_queue.transcode_completions_eventbridge_dlq.arn
  }
}

# EventBridge-delivery DLQ for the MediaConvert completions rule target (see the target above).
resource "aws_sqs_queue" "transcode_completions_eventbridge_dlq" {
  name                      = "${var.name}-transcode-completions-eventbridge-dlq"
  message_retention_seconds = 1209600 # 14 days
  tags                      = var.tags
}

# Allow EventBridge to send the failed completion events to the delivery DLQ, scoped to this rule.
resource "aws_sqs_queue_policy" "transcode_completions_eventbridge_dlq" {
  queue_url = aws_sqs_queue.transcode_completions_eventbridge_dlq.url

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "events.amazonaws.com" }
      Action    = "sqs:SendMessage"
      Resource  = aws_sqs_queue.transcode_completions_eventbridge_dlq.arn
      Condition = {
        ArnEquals = { "aws:SourceArn" = aws_cloudwatch_event_rule.mediaconvert_jobs.arn }
      }
    }]
  })
}

resource "aws_sqs_queue_policy" "transcode_completions" {
  queue_url = aws_sqs_queue.transcode_completions.url

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "events.amazonaws.com" }
      Action    = "sqs:SendMessage"
      Resource  = aws_sqs_queue.transcode_completions.arn
      Condition = {
        ArnEquals = { "aws:SourceArn" = aws_cloudwatch_event_rule.mediaconvert_jobs.arn }
      }
    }]
  })
}

# --- OpenSearch (regional; domain name is region-free since domains live in regional namespaces) ---

# Look up the VPC CIDR so the domain's security group can admit HTTPS from within the VPC only.
data "aws_vpc" "this" {
  id = var.vpc_id
}

# Security group for the VPC-attached OpenSearch domain. The domain's ENIs sit in a private subnet;
# admit only HTTPS (443) from inside the VPC. The search pods (VPC-CNI IPs in the VPC CIDR) reach the
# domain entirely on the AWS private network — no NAT gateway / public-internet hop. That public path
# is what made cold (hourly-idle) TLS handshakes to the domain intermittently stall 12-24s and trip
# the shallow canary; a private ENI path removes it at the source.
resource "aws_security_group" "opensearch" {
  name        = "${var.name}-opensearch"
  description = "OpenSearch ${var.name}-search - HTTPS from within the VPC only"
  vpc_id      = var.vpc_id

  ingress {
    description = "HTTPS from within the VPC (EKS pods)"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = [data.aws_vpc.this.cidr_block]
  }

  egress {
    from_port   = 0
    to_port     = 0
    protocol    = "-1"
    cidr_blocks = ["0.0.0.0/0"]
  }

  tags = merge(var.tags, { Name = "${var.name}-opensearch" })
}

resource "aws_opensearch_domain" "this" {
  domain_name    = "${var.name}-search"
  engine_version = "OpenSearch_2.11"

  cluster_config {
    # t3.medium (not t3.small): the small's ~1GB heap ran chronically at 65-74% JVM memory pressure,
    # grazing the GC threshold, so the single node periodically stalled for tens of seconds and hung
    # /search calls until the search client's timeout -> 500 -> shallow-canary flap. t3.medium doubles
    # the heap (~35% steady pressure) and drops burstable-CPU variability. Still single-node / single-AZ
    # by design (see vpc_options + zone awareness below) — the redundancy gap is intentionally retained
    # for resilience analysis; this bump only removes the incidental undersizing brownout.
    instance_type  = "t3.medium.search"
    instance_count = 1
  }

  # VPC-attached: ENIs live in a private subnet, reachable only on the AWS private network (pod ->
  # ENI). This replaces the public endpoint path (pod -> NAT GW -> internet -> ES public front-end),
  # whose new-connection TLS handshakes intermittently stalled 12-24s and failed the shallow canary's
  # /search step. Single-node domain (zone awareness disabled) => exactly ONE subnet / one AZ, which
  # matches the existing single-AZ search posture (DESIGN §10.7).
  # NOTE: public <-> VPC is NOT an in-place change — this forces a domain REPLACEMENT (destroy+create)
  # and a new (vpc-…) endpoint. The index is rebuildable per region (scripts/reindex.sh), so re-seed
  # after cutover. Requires the account-global service-linked role
  # AWSServiceRoleForAmazonOpenSearchService to exist (one-time:
  # `aws iam create-service-linked-role --aws-service-name opensearchservice.amazonaws.com`).
  vpc_options {
    subnet_ids         = [var.private_subnet_ids[0]]
    security_group_ids = [aws_security_group.opensearch.id]
  }

  ebs_options {
    ebs_enabled = true
    volume_size = 20
    volume_type = "gp3"
  }

  node_to_node_encryption {
    enabled = true
  }

  encrypt_at_rest {
    enabled = true
  }

  domain_endpoint_options {
    enforce_https       = true
    tls_security_policy = "Policy-Min-TLS-1-2-2019-07"
  }

  advanced_security_options {
    enabled                        = true
    internal_user_database_enabled = false

    master_user_options {
      master_user_arn = var.search_role_arn
    }
  }

  access_policies = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { AWS = "*" }
      Action    = "es:*"
      Resource  = "arn:aws:es:${var.region}:*:domain/${var.name}-search/*"
    }]
  })

  tags = var.tags
}

# --- Outputs (mirror the old data module so env consumers are unchanged) ---

output "s3_bucket_arns" {
  value = {
    raw    = aws_s3_bucket.raw.arn
    videos = aws_s3_bucket.videos.arn
  }
}

output "videos_bucket_id" {
  value = aws_s3_bucket.videos.id
}

output "videos_bucket_regional_domain_name" {
  value = aws_s3_bucket.videos.bucket_regional_domain_name
}

output "sqs_queue_arn" {
  value = aws_sqs_queue.transcode.arn
}

output "sqs_queue_url" {
  value = aws_sqs_queue.transcode.url
}

output "search_index_queue_arn" {
  value = aws_sqs_queue.search_index_events.arn
}

output "search_index_queue_url" {
  value = aws_sqs_queue.search_index_events.url
}

output "delete_cleanup_queue_arn" {
  value = aws_sqs_queue.delete_cleanup_events.arn
}

output "delete_cleanup_queue_url" {
  value = aws_sqs_queue.delete_cleanup_events.url
}

output "transcode_completions_queue_arn" {
  value = aws_sqs_queue.transcode_completions.arn
}

output "transcode_completions_queue_url" {
  value = aws_sqs_queue.transcode_completions.url
}

output "mediaconvert_role_arn" {
  value = aws_iam_role.mediaconvert.arn
}

output "opensearch_endpoint" {
  value = aws_opensearch_domain.this.endpoint
}
