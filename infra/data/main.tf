# infra/data — region-neutral root stack that OWNS the DynamoDB tables (the DynamoDB Global Table
# origin). Extracted from the dev/us-west-2 env so that neither regional
# environment owns the data both share: each regional env reads table ARNs (and, later, its local
# replica stream ARN) from this stack's remote state. Multi-region replicas are added here, in the
# global-data module (replica blocks), at expansion.
#
# The tables already exist (originally created by the dev/us-west-2 env). They are migrated into
# THIS stack's state via the `import` blocks below — a metadata-only move; the live tables and their
# data are untouched. The import blocks can be deleted after the one-time import.

terraform {
  required_version = ">= 1.5"

  backend "s3" {
    bucket         = "rewind-terraform-state"
    key            = "data/terraform.tfstate"
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

# DynamoDB Global Table replica regions. Empty in single-region (no replicas). At multi-region
# expansion set this to ["us-east-2"] and apply THIS stack FIRST:
# the replica must exist before the us-east-2 env's Pipes can resolve their local replica stream ARN.
variable "replica_regions" {
  description = "Regions to add as DynamoDB Global Table replicas (empty = single-region)"
  type        = list(string)
  default     = []
}

# Home region of the tables (the Global Table origin). Replicas in other regions are added via
# replica blocks in the global-data module at multi-region expansion.
provider "aws" {
  region  = "us-west-2"
  profile = var.aws_profile

  default_tags {
    tags = {
      Project     = "rewind"
      ManagedBy   = "terraform"
      Environment = var.environment
      Component   = "data"
    }
  }
}

locals {
  name = "rewind-${var.environment}"
}

module "global_data" {
  source          = "../modules/global-data"
  name            = local.name
  replica_regions = var.replica_regions
}

# --- Outputs (consumed by the regional envs via terraform_remote_state) ---
output "dynamodb_table_arns" {
  value = module.global_data.dynamodb_table_arns
}

output "videos_stream_arn" {
  value = module.global_data.videos_stream_arn
}
