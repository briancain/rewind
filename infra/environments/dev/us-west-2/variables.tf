variable "aws_profile" {
  description = "AWS CLI profile name"
  type        = string
  default     = "rewind"
}

variable "region" {
  description = "AWS region"
  type        = string
  default     = "us-west-2"
}

variable "environment" {
  description = "Environment name"
  type        = string
  default     = "dev"
}

variable "admin_role_arn" {
  description = "IAM role ARN for EKS cluster admin access. Set in terraform.tfvars (account-specific)."
  type        = string
}

variable "alert_email" {
  description = "Email address that receives CloudWatch alarm notifications (SNS). Set in terraform.tfvars."
  type        = string
}

# Whether bidirectional S3 CRR is enabled for this region. The CRR *peer* itself is derived from the
# region topology in locals (not set here), so it can never be wrong. Defaults to true so a routine
# `terraform apply` always preserves replication — there is no way to silently disable it. Set false
# ONLY for the one-time cold bootstrap of a region before its peer's buckets exist,
# then restore via a reviewed commit.
variable "enable_replication" {
  description = "Enable bidirectional S3 CRR for this region (peer derived from topology). Set false only for initial cold bootstrap before the peer region exists."
  type        = bool
  default     = true
}
