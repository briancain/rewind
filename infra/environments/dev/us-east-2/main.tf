terraform {
  required_version = ">= 1.5"

  backend "s3" {
    bucket         = "rewind-terraform-state"
    key            = "dev/us-east-2/terraform.tfstate"
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
    kubernetes = {
      source  = "hashicorp/kubernetes"
      version = "~> 2.0"
    }
  }
}

provider "aws" {
  region  = var.region
  profile = var.aws_profile

  default_tags {
    tags = {
      Project     = "rewind"
      ManagedBy   = "terraform"
      Environment = var.environment
      Region      = var.region
    }
  }
}

locals {
  name = "rewind-${var.environment}"
  # Region-qualified prefix for IAM roles (account-global namespace) so two regions don't collide.
  regional_name = "rewind-${var.environment}-${var.region}"
  azs           = ["${var.region}a", "${var.region}b", "${var.region}c"]
  domain        = data.terraform_remote_state.global.outputs.domain
  cdn_domain    = "cdn.${local.domain}"

  # Active/active CRR topology: each region's replication peer, derived (not an operator input) so
  # the peer identity can never be forgotten or mistyped — adding a region means extending this map.
  peer_region_map = {
    "us-west-2" = "us-east-2"
    "us-east-2" = "us-west-2"
  }
  # Effective CRR peer passed to the modules (which gate on a non-empty peer): the derived peer when
  # replication is enabled, else "" to disable CRR. var.enable_replication defaults to true, so a
  # bare apply always preserves replication; it's set false only for the one-time cold bootstrap of
  # a region before its peer's buckets exist, then restored via a reviewed commit.
  peer_region = var.enable_replication ? local.peer_region_map[var.region] : ""

  # The 11 DynamoDB tables are Global Tables owned by the region-neutral infra/data stack. Their ARNs
  # are region-qualified, so this region's pods must be granted access to THIS region's replica ARNs
  # (arn:aws:dynamodb:us-east-2:...), NOT the home stack's us-west-2 ARNs. The table NAMES are
  # region-free (a Global Table shares one name), so we derive the local replica ARNs by convention.
  table_names = [
    "users", "sessions", "videos", "comments", "reactions", "video_stats",
    "comment_reactions", "view_history", "invite_codes", "verification_tokens", "transcode_jobs",
  ]
  dynamodb_table_arns = {
    for t in local.table_names :
    t => "arn:aws:dynamodb:${var.region}:${data.aws_caller_identity.current.account_id}:table/${local.name}-${t}"
  }
}

module "vpc" {
  source = "../../../modules/vpc"

  name   = local.name
  region = var.region
  azs    = local.azs
  # Distinct CIDR from us-west-2 (10.0.0.0/16) — no peering in this design, but distinct ranges are
  # free future-proofing.
  cidr = "10.1.0.0/16"

  public_subnets  = ["10.1.1.0/24", "10.1.2.0/24", "10.1.3.0/24"]
  private_subnets = ["10.1.10.0/24", "10.1.11.0/24", "10.1.12.0/24"]
}

module "eks" {
  source = "../../../modules/eks"

  name           = local.name
  iam_name       = local.regional_name
  subnet_ids     = module.vpc.private_subnet_ids
  admin_role_arn = var.admin_role_arn
}

module "ecr" {
  source = "../../../modules/ecr"

  name_prefix = local.name
  services = [
    "identity",
    "video-catalog",
    "upload",
    "transcode",
    "streaming",
    "social",
    "search",
    "delete-cleanup",
    "canary",
    "frontend",
  ]
}

data "aws_caller_identity" "current" {}

data "terraform_remote_state" "global" {
  backend = "s3"
  config = {
    bucket  = "rewind-terraform-state"
    key     = "global/terraform.tfstate"
    region  = "us-west-2"
    profile = var.aws_profile
  }
}

# This region's LOCAL replica of the videos Global Table. The home infra/data stack's `replica`
# blocks create it; the home table resource does NOT expose per-replica stream ARNs, so we look the
# replica up here (in this region's provider) to feed its stream to the regional Pipes. This lookup
# REQUIRES the replica to already exist — apply infra/data with replica_regions including this region
# FIRST. It also avoids a remote-state dependency on the home
# env and is the natural ordering guard.
data "aws_dynamodb_table" "videos" {
  name = "${local.name}-videos"
}

module "regional_data" {
  source = "../../../modules/regional-data"

  name               = local.name
  iam_name           = local.regional_name
  region             = var.region
  vpc_id             = module.vpc.vpc_id
  private_subnet_ids = module.vpc.private_subnet_ids
  search_role_arn    = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:role/${local.regional_name}-search"
  videos_stream_arn  = data.aws_dynamodb_table.videos.stream_arn

  # Bidirectional S3 CRR back to the home region. Empty by default (set at expansion).
  peer_region = local.peer_region
}

# --- ALB ACM certificate (regional) ---
# ACM certs are regional and an ALB can only use a cert in its own region, so this region needs its
# own wildcard cert (the global stack's cert is us-west-2-only). Same domain + wildcard, DNS-validated
# against the shared hosted zone. allow_overwrite on the validation records because they share the
# same names as the us-west-2 global cert's validation records for this domain (ACM reuses the CNAME
# per domain across certs in an account) — overwriting an identical record is idempotent.
resource "aws_acm_certificate" "alb" {
  domain_name               = local.domain
  subject_alternative_names = ["*.${local.domain}"]
  validation_method         = "DNS"

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "cert_validation" {
  for_each = {
    for dvo in aws_acm_certificate.alb.domain_validation_options : dvo.domain_name => {
      name   = dvo.resource_record_name
      record = dvo.resource_record_value
      type   = dvo.resource_record_type
    }
  }

  zone_id         = data.terraform_remote_state.global.outputs.hosted_zone_id
  name            = each.value.name
  type            = each.value.type
  records         = [each.value.record]
  ttl             = 60
  allow_overwrite = true
}

resource "aws_acm_certificate_validation" "alb" {
  certificate_arn         = aws_acm_certificate.alb.arn
  validation_record_fqdns = [for r in aws_route53_record.cert_validation : r.fqdn]
}

# --- DNS Records (created after ALB exists from Ingress) ---

data "aws_lb" "ingress" {
  tags = {
    "eks:eks-cluster-name" = local.name
  }

  depends_on = [kubernetes_manifest.ingress]
}

# Region-pinned hostname (NOT latency-routed) → this region's ALB, probed by the Route 53 health check.
resource "aws_route53_record" "region_pinned" {
  zone_id = data.terraform_remote_state.global.outputs.hosted_zone_id
  name    = "${var.region}.${local.domain}"
  type    = "A"

  alias {
    name                   = data.aws_lb.ingress.dns_name
    zone_id                = data.aws_lb.ingress.zone_id
    evaluate_target_health = true
  }
}

# Health check for THIS region's stack (HTTPS GET /api/health on the region-pinned frontend host).
# A regional outage fails this check, removing this region from the latency records so Route 53
# directs users to the surviving region.
resource "aws_route53_health_check" "regional" {
  type              = "HTTPS"
  fqdn              = "${var.region}.${local.domain}"
  port              = 443
  resource_path     = "/api/health"
  failure_threshold = 3
  request_interval  = 30

  tags = { Name = "${local.regional_name}-health" }
}

# Latency-routed apex + wildcard → this region's ALB. `set_identifier` per region makes these purely
# additive to the us-west-2 records already in the shared zone, and the health check enables
# automatic fail-away.
resource "aws_route53_record" "alb" {
  zone_id        = data.terraform_remote_state.global.outputs.hosted_zone_id
  name           = local.domain
  type           = "A"
  set_identifier = var.region

  latency_routing_policy {
    region = var.region
  }

  health_check_id = aws_route53_health_check.regional.id

  alias {
    name                   = data.aws_lb.ingress.dns_name
    zone_id                = data.aws_lb.ingress.zone_id
    evaluate_target_health = true
  }
}

resource "aws_route53_record" "alb_wildcard" {
  zone_id        = data.terraform_remote_state.global.outputs.hosted_zone_id
  name           = "*.${local.domain}"
  type           = "A"
  set_identifier = var.region

  latency_routing_policy {
    region = var.region
  }

  health_check_id = aws_route53_health_check.regional.id

  alias {
    name                   = data.aws_lb.ingress.dns_name
    zone_id                = data.aws_lb.ingress.zone_id
    evaluate_target_health = true
  }
}

# --- WAF on the regional ALB (rate limiting + AWS managed protections) ---
module "waf" {
  source = "../../../modules/waf"

  name    = local.name
  alb_arn = data.aws_lb.ingress.arn
}

module "irsa" {
  source = "../../../modules/irsa"

  name              = local.regional_name
  oidc_provider_arn = module.eks.oidc_provider_arn
  oidc_provider_url = module.eks.oidc_provider_url
  # Region-local replica ARNs (see local.dynamodb_table_arns) so pods are granted access to THIS
  # region's table replicas, not the home region's.
  dynamodb_table_arns    = local.dynamodb_table_arns
  s3_bucket_arns         = module.regional_data.s3_bucket_arns
  sqs_queue_arn          = module.regional_data.sqs_queue_arn
  search_index_queue_arn = module.regional_data.search_index_queue_arn

  mediaconvert_role_arn           = module.regional_data.mediaconvert_role_arn
  transcode_completions_queue_arn = module.regional_data.transcode_completions_queue_arn
  delete_cleanup_queue_arn        = module.regional_data.delete_cleanup_queue_arn
}

module "observability" {
  source = "../../../modules/observability"

  name                       = local.name
  region                     = var.region
  alb_arn_suffix             = data.aws_lb.ingress.arn_suffix
  alert_email                = var.alert_email
  sqs_queue_name             = "${local.name}-transcode-jobs"
  search_index_queue_name    = "${local.name}-search-index-events.fifo"
  search_index_dlq_name      = "${local.name}-search-index-events-dlq.fifo"
  videos_to_search_pipe_name = "${local.name}-videos-to-search"
  delete_cleanup_dlq_name    = "${local.name}-delete-cleanup-dlq.fifo"
  opensearch_domain_name     = "${local.name}-search"
  log_group_prefix           = "/aws/containerinsights/${local.name}/application"

  transcode_completions_eventbridge_dlq_name = "${local.name}-transcode-completions-eventbridge-dlq"

  # S3 CRR replication alarms — created only when replication is enabled (mirrors regional-data).
  crr_peer_region = local.peer_region

  # Canary freshness ("is it running?") alarms — only for scheduled tiers. shallow is enabled; deep
  # stays suspended (add `deep = 86400` here when you un-suspend deep). Interval seconds = the cadence.
  canary_freshness = { shallow = 3600 }
}

provider "kubernetes" {
  host                   = module.eks.cluster_endpoint
  cluster_ca_certificate = base64decode(module.eks.cluster_ca_certificate)

  exec {
    api_version = "client.authentication.k8s.io/v1beta1"
    command     = "aws"
    args        = ["eks", "get-token", "--cluster-name", module.eks.cluster_name, "--region", var.region, "--profile", var.aws_profile]
  }
}

resource "kubernetes_namespace" "rewind" {
  metadata {
    name = "rewind"
  }
}

resource "kubernetes_manifest" "ingress_class_params" {
  manifest = {
    apiVersion = "eks.amazonaws.com/v1"
    kind       = "IngressClassParams"
    metadata = {
      name = "alb"
    }
    spec = {
      scheme = "internet-facing"
      # This region's own regional ACM cert (NOT the us-west-2 global cert).
      certificateARNs = [aws_acm_certificate_validation.alb.certificate_arn]
    }
  }
}

resource "kubernetes_manifest" "ingress_class" {
  manifest = {
    apiVersion = "networking.k8s.io/v1"
    kind       = "IngressClass"
    metadata = {
      name = "alb"
      annotations = {
        "ingressclass.kubernetes.io/is-default-class" = "true"
      }
    }
    spec = {
      controller = "eks.amazonaws.com/alb"
      parameters = {
        apiGroup = "eks.amazonaws.com"
        kind     = "IngressClassParams"
        name     = "alb"
      }
    }
  }
}

resource "kubernetes_manifest" "ingress" {
  manifest = {
    apiVersion = "networking.k8s.io/v1"
    kind       = "Ingress"
    metadata = {
      name      = "rewind"
      namespace = "rewind"
      annotations = {
        # All services (8 backends + frontend) serve /health -> 200. Without this the ALB target
        # groups default to checking "/" (matcher 200), which the Rust backends answer with 404, so
        # every backend target sits permanently "unhealthy" and only works via ALB fail-open.
        "alb.ingress.kubernetes.io/healthcheck-path" = "/health"
      }
    }
    spec = {
      ingressClassName = "alb"
      rules = [
        {
          host = "identity.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "identity"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          host = "catalog.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "video-catalog"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          host = "upload.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "upload"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          host = "streaming.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "streaming"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          host = "social.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "social"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          host = "search.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "search"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          host = "transcode.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "transcode"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          host = local.domain
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "frontend"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
        {
          # Region-pinned host → frontend, so the Route 53 health check can probe /api/health on a
          # stable per-region address (see aws_route53_health_check.regional).
          host = "${var.region}.${local.domain}"
          http = {
            paths = [{
              path     = "/*"
              pathType = "ImplementationSpecific"
              backend = {
                service = {
                  name = "frontend"
                  port = { number = 3000 }
                }
              }
            }]
          }
        },
      ]
    }
  }
}

resource "kubernetes_manifest" "nodepool_arm64" {
  manifest = {
    apiVersion = "karpenter.sh/v1"
    kind       = "NodePool"
    metadata = {
      name = "arm64"
    }
    spec = {
      template = {
        spec = {
          nodeClassRef = {
            group = "eks.amazonaws.com"
            kind  = "NodeClass"
            name  = "default"
          }
          requirements = [
            {
              key      = "kubernetes.io/arch"
              operator = "In"
              values   = ["arm64"]
            },
            {
              key      = "karpenter.sh/capacity-type"
              operator = "In"
              values   = ["on-demand"]
            },
            {
              key      = "eks.amazonaws.com/instance-category"
              operator = "In"
              values   = ["c", "m", "r"]
            },
            {
              key      = "eks.amazonaws.com/instance-generation"
              operator = "Gt"
              values   = ["4"]
            },
          ]
        }
      }
      disruption = {
        consolidationPolicy = "WhenEmptyOrUnderutilized"
        # Less aggressive than 30s to cut consolidation churn, and cap voluntary disruption to ONE
        # node at a time (drift + consolidation) so a node rotation can't drain multiple nodes' pods
        # at once. With per-service PodDisruptionBudgets this keeps node rotations graceful.
        consolidateAfter = "5m"
        budgets = [
          { nodes = "1" }
        ]
      }
      # Headroom for a temporary replacement node during a graceful rotation. limits is a cap, not a
      # request — Karpenter still launches only what's needed, so raising it adds no steady-state cost.
      limits = {
        cpu = "16"
      }
    }
  }
}
