# global-data — the DynamoDB tables, which become DynamoDB Global Tables at multi-region expansion.
#
# These are deliberately separated from regional-data (S3/SQS/OpenSearch/etc.) because Global Tables
# must be owned in exactly ONE place: the home region adds `replica` blocks here to create the
# secondary-region replicas, and the per-region env must NOT re-create same-named tables. Table names are therefore REGION-FREE (a Global Table shares one name across regions).
#
# Every table has a stream enabled (NEW_AND_OLD_IMAGES): the `videos` stream already feeds the
# search + cascade-cleanup pipelines, and Global Tables requires streams on every replicated table.
# Enabling streams now (single-region) is non-destructive and a prerequisite for the replica blocks.

variable "name" {
  description = "Region-free name prefix for tables (shared across Global Table replicas), e.g. rewind-dev"
  type        = string
}

variable "tags" {
  description = "Additional tags"
  type        = map(string)
  default     = {}
}

# Regions to add as DynamoDB Global Table replicas. Empty in single-region (no replicas, no change);
# set to e.g. ["us-east-2"] at multi-region expansion, applied from THIS (home) stack so the tables
# are owned in exactly one place. Adding a region makes AWS create the replica
# and copy existing data automatically. Streams are already enabled (required for Global Tables).
variable "replica_regions" {
  description = "Regions to add as DynamoDB Global Table replicas (empty = single-region)"
  type        = list(string)
  default     = []
}

locals {
  # Stream view type required by both the existing pipelines and DynamoDB Global Tables.
  stream_view_type = "NEW_AND_OLD_IMAGES"
}

resource "aws_dynamodb_table" "users" {
  name             = "${var.name}-users"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "user_id"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "user_id"
    type = "S"
  }

  attribute {
    name = "email"
    type = "S"
  }

  global_secondary_index {
    name            = "email-index"
    hash_key        = "email"
    projection_type = "ALL"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "sessions" {
  name             = "${var.name}-sessions"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "session_token"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "session_token"
    type = "S"
  }

  attribute {
    name = "user_id"
    type = "S"
  }

  # Lets the identity service find all of a user's sessions (to invalidate them on password change).
  global_secondary_index {
    name            = "user-id-index"
    hash_key        = "user_id"
    projection_type = "KEYS_ONLY"
  }

  # TTL attribute matches what the identity service writes (epoch-seconds Number named `ttl`) and
  # what shared::auth reads for app-level expiry.
  ttl {
    attribute_name = "ttl"
    enabled        = true
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "videos" {
  name             = "${var.name}-videos"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "video_id"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  # Finalizer: the cascade soft-delete stamps a numeric `purge_at` (epoch seconds) on
  # the tombstone; TTL hard-deletes the row after it passes.
  ttl {
    attribute_name = "purge_at"
    enabled        = true
  }

  attribute {
    name = "video_id"
    type = "S"
  }

  attribute {
    name = "status"
    type = "S"
  }

  attribute {
    name = "channel_id"
    type = "S"
  }

  global_secondary_index {
    name            = "status-index"
    hash_key        = "status"
    projection_type = "ALL"
  }

  global_secondary_index {
    name            = "channel-index"
    hash_key        = "channel_id"
    projection_type = "ALL"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "comments" {
  name             = "${var.name}-comments"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "video_id"
  range_key        = "comment_id"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "video_id"
    type = "S"
  }

  attribute {
    name = "comment_id"
    type = "S"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "reactions" {
  name             = "${var.name}-reactions"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "video_id"
  range_key        = "user_id"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "video_id"
    type = "S"
  }

  attribute {
    name = "user_id"
    type = "S"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "video_stats" {
  name             = "${var.name}-video_stats"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "video_id"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "video_id"
    type = "S"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "comment_reactions" {
  name             = "${var.name}-comment_reactions"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "video_id"
  range_key        = "sk"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "video_id"
    type = "S"
  }

  attribute {
    name = "sk"
    type = "S"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "view_history" {
  name             = "${var.name}-view_history"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "user_id"
  range_key        = "watched_at"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "user_id"
    type = "S"
  }

  attribute {
    name = "watched_at"
    type = "S"
  }

  attribute {
    name = "video_id"
    type = "S"
  }

  # view_history is user-scoped (PK=user_id); this GSI lets the cascade cleanup find all entries
  # for a deleted video. KEYS_ONLY — cleanup only needs the base keys to delete.
  global_secondary_index {
    name            = "video-id-index"
    hash_key        = "video_id"
    projection_type = "KEYS_ONLY"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "invite_codes" {
  name             = "${var.name}-invite_codes"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "code"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "code"
    type = "S"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "verification_tokens" {
  name             = "${var.name}-verification_tokens"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "token"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "token"
    type = "S"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

resource "aws_dynamodb_table" "transcode_jobs" {
  name             = "${var.name}-transcode_jobs"
  billing_mode     = "PAY_PER_REQUEST"
  hash_key         = "job_id"
  stream_enabled   = true
  stream_view_type = local.stream_view_type

  attribute {
    name = "job_id"
    type = "S"
  }

  attribute {
    name = "video_id"
    type = "S"
  }

  global_secondary_index {
    name            = "video-id-index"
    hash_key        = "video_id"
    projection_type = "ALL"
  }

  dynamic "replica" {
    for_each = toset(var.replica_regions)
    content {
      region_name = replica.value
    }
  }

  tags = var.tags
}

# --- Outputs ---

output "dynamodb_table_arns" {
  value = {
    users               = aws_dynamodb_table.users.arn
    sessions            = aws_dynamodb_table.sessions.arn
    videos              = aws_dynamodb_table.videos.arn
    comments            = aws_dynamodb_table.comments.arn
    reactions           = aws_dynamodb_table.reactions.arn
    video_stats         = aws_dynamodb_table.video_stats.arn
    comment_reactions   = aws_dynamodb_table.comment_reactions.arn
    view_history        = aws_dynamodb_table.view_history.arn
    invite_codes        = aws_dynamodb_table.invite_codes.arn
    verification_tokens = aws_dynamodb_table.verification_tokens.arn
    transcode_jobs      = aws_dynamodb_table.transcode_jobs.arn
  }
}

# The home-region videos stream ARN. The search + cascade-cleanup Pipes in THIS region consume it.
# At multi-region expansion, each region's regional-data consumes its own local replica stream ARN
# (looked up via a `data "aws_dynamodb_table"` in the secondary env), not this one.
output "videos_stream_arn" {
  value = aws_dynamodb_table.videos.stream_arn
}
