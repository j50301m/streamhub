terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 6.12"
    }
  }

  backend "gcs" {}
}

# NOTE: per SPEC-011a, staging reuses the prod regional cluster via namespace isolation.
# This root only provisions staging-scoped resources (GCS buckets, AR repo if separate project).
# If staging runs in the same project as prod, consume envs/prod outputs via a data source instead
# of recreating the cluster. This file keeps a minimal staging-specific footprint.

provider "google" {
  project = var.project_id
  region  = var.region
}

provider "google-beta" {
  project = var.project_id
  region  = var.region
}

locals {
  env = "staging"
}

resource "google_compute_global_address" "gateway_public" {
  project = var.project_id
  name    = "streamhub-staging-public"
}

module "certs" {
  source = "../../modules/certs"

  project_id    = var.project_id
  cert_map_name = "streamhub-staging-cert-map"

  certificates = {
    api = {
      certificate_name = "streamhub-api-cert-staging"
      hostname         = var.domain_staging
    }
    admin = {
      certificate_name = "streamhub-admin-cert-staging"
      hostname         = var.admin_domain_staging
    }
    web = {
      certificate_name = "streamhub-web-cert-staging"
      hostname         = var.web_domain_staging
    }
  }
}

module "gcs" {
  source = "../../modules/gcs"

  project_id = var.project_id

  buckets = {
    recordings = {
      name               = "streamhub-recordings-${local.env}"
      lifecycle_age_days = 7
      lifecycle_action   = "Delete"
      force_destroy      = true
    }
    vod = {
      name          = "streamhub-vod-${local.env}"
      force_destroy = true
    }
    thumbnails = {
      name          = "streamhub-thumbnails-${local.env}"
      force_destroy = true
    }
  }
}

# Cloud Armor policies for the staging surface. Names are suffixed with -staging so they
# can coexist with prod policies in the same project. The staging Kustomize overlay
# patches GCPBackendPolicy to reference these names.
module "cloud_armor_public" {
  source = "../../modules/cloud-armor"

  project_id  = var.project_id
  name        = "streamhub-public-armor-${local.env}"
  description = "Cloud Armor policy for staging public workloads"

  enable_rate_based_ban    = true
  rate_limit_count         = 100
  rate_limit_interval_sec  = 60
  rate_ban_duration_sec    = 600
  enable_waf_preconfigured = true
  enable_ip_allowlist      = false
  ip_allowlist             = []
}

module "cloud_armor_admin" {
  source = "../../modules/cloud-armor"

  project_id  = var.project_id
  name        = "streamhub-admin-armor-${local.env}"
  description = "Cloud Armor policy for staging admin surface"

  enable_rate_based_ban    = true
  rate_limit_count         = 30
  rate_limit_interval_sec  = 60
  rate_ban_duration_sec    = 1800
  enable_waf_preconfigured = true
  enable_ip_allowlist      = false
  ip_allowlist             = []
}

output "gcs_buckets" {
  value = module.gcs.bucket_names
}

output "cloud_armor_public_name" {
  value = module.cloud_armor_public.name
}

output "cloud_armor_admin_name" {
  value = module.cloud_armor_admin.name
}

output "certificate_map_name" {
  value = module.certs.certificate_map_name
}

output "gateway_public_address_name" {
  value = google_compute_global_address.gateway_public.name
}
