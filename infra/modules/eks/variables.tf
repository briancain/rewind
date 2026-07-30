variable "name" {
  description = "Cluster name (region-free; EKS cluster names live in a regional namespace)"
  type        = string
}

variable "iam_name" {
  description = "Region-qualified prefix for the cluster/node IAM roles (account-global namespace)"
  type        = string
}

variable "kubernetes_version" {
  description = "Kubernetes version"
  type        = string
  default     = "1.31"
}

variable "subnet_ids" {
  description = "Subnet IDs for the EKS cluster (private subnets)"
  type        = list(string)
}

variable "tags" {
  description = "Additional tags"
  type        = map(string)
  default     = {}
}

variable "admin_role_arn" {
  description = "IAM role ARN to grant cluster admin access"
  type        = string
}
