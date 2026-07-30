output "vpc_id" {
  value = module.vpc.vpc_id
}

output "region" {
  value = var.region
}

output "private_subnet_ids" {
  value = module.vpc.private_subnet_ids
}

output "public_subnet_ids" {
  value = module.vpc.public_subnet_ids
}

output "cluster_name" {
  value = module.eks.cluster_name
}

output "cluster_endpoint" {
  value = module.eks.cluster_endpoint
}

output "ecr_repository_urls" {
  value = module.ecr.repository_urls
}

output "opensearch_endpoint" {
  value = module.regional_data.opensearch_endpoint
}

output "sqs_queue_url" {
  value = module.regional_data.sqs_queue_url
}

output "search_index_queue_url" {
  value = module.regional_data.search_index_queue_url
}

output "delete_cleanup_queue_url" {
  value = module.regional_data.delete_cleanup_queue_url
}

output "dynamodb_table_prefix" {
  value = "${local.name}-"
}

output "s3_bucket_raw" {
  value = "${local.name}-raw-${var.region}"
}

output "s3_bucket_videos" {
  value = "${local.name}-videos-${var.region}"
}

output "service_role_arns" {
  value = module.irsa.role_arns
}

output "domain" {
  value = local.domain
}

output "cdn_domain" {
  value = local.cdn_domain
}

# Consumed by the infra/cdn stack (via remote state) to wire the CloudFront origin + bucket policy.
output "videos_bucket_id" {
  value = module.regional_data.videos_bucket_id
}

output "videos_bucket_arn" {
  value = module.regional_data.s3_bucket_arns["videos"]
}

output "videos_bucket_regional_domain_name" {
  value = module.regional_data.videos_bucket_regional_domain_name
}

output "mediaconvert_role_arn" {
  value = module.regional_data.mediaconvert_role_arn
}

output "transcode_completions_queue_url" {
  value = module.regional_data.transcode_completions_queue_url
}

# Consumed by the global infra/observability stack (via remote state) to render this region's ALB
# latency widget. arn_suffix is exactly the CloudWatch "LoadBalancer" dimension (app/<name>/<id>).
output "alb_arn_suffix" {
  value = data.aws_lb.ingress.arn_suffix
}
