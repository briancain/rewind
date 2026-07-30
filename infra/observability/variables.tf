variable "name" {
  description = "Region-free resource/name prefix (matches the regional stacks' local.name)"
  type        = string
  default     = "rewind-dev"
}

variable "regions" {
  description = "Active regions to render on the dashboard, in display order. Each must have an applied dev/<region> env state (its alb_arn_suffix output is read via remote state)."
  type        = list(string)
  default     = ["us-west-2", "us-east-2"]

  validation {
    condition     = length(var.regions) > 0
    error_message = "At least one region is required."
  }
}

variable "state_bucket" {
  description = "S3 bucket holding the per-region env Terraform state (for remote-state reads)"
  type        = string
  default     = "rewind-terraform-state"
}

variable "state_region" {
  description = "Region of the Terraform state bucket"
  type        = string
  default     = "us-west-2"
}

variable "aws_profile" {
  description = "AWS CLI profile name"
  type        = string
  default     = "rewind"
}
