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
    helm = {
      source  = "hashicorp/helm"
      version = "~> 2.16"
    }
  }

  # Configure the GCS state backend per-project (values filled via `terraform init -backend-config=...`).
  backend "gcs" {}
}

provider "google" {
  project = var.project_id
  region  = var.region
}

provider "google-beta" {
  project = var.project_id
  region  = var.region
}

data "google_client_config" "current" {}

provider "helm" {
  kubernetes {
    host                   = "https://${module.cluster.cluster_endpoint}"
    token                  = data.google_client_config.current.access_token
    cluster_ca_certificate = base64decode(module.cluster.cluster_ca_certificate)
  }
}

locals {
  env = "prod"

  required_apis = [
    "compute.googleapis.com",
    "container.googleapis.com",
    "servicenetworking.googleapis.com",
    "sqladmin.googleapis.com",
    "redis.googleapis.com",
    "artifactregistry.googleapis.com",
    "secretmanager.googleapis.com",
    "certificatemanager.googleapis.com",
    "dns.googleapis.com",
    "iam.googleapis.com",
    "iamcredentials.googleapis.com",
    "monitoring.googleapis.com",
    "logging.googleapis.com",
    "cloudtrace.googleapis.com",
    "pubsub.googleapis.com",
    "transcoder.googleapis.com",
  ]
}

resource "google_project_service" "enabled" {
  for_each = toset(local.required_apis)

  project            = var.project_id
  service            = each.value
  disable_on_destroy = false
}

module "network" {
  source = "../../modules/network"

  project_id = var.project_id
  region     = var.region

  depends_on = [google_project_service.enabled]
}

module "cluster" {
  source = "../../modules/cluster"

  project_id                 = var.project_id
  region                     = var.region
  cluster_name               = "streamhub-${local.env}"
  network_self_link          = module.network.network_self_link
  subnet_self_link           = module.network.subnet_self_link
  pods_range_name            = module.network.pods_range_name
  services_range_name        = module.network.services_range_name
  master_ipv4_cidr           = module.network.master_ipv4_cidr
  master_authorized_networks = var.master_authorized_networks

  app_pool_min_nodes_per_zone = 1
  app_pool_max_nodes_per_zone = 4
}

module "database" {
  source = "../../modules/database"

  project_id        = var.project_id
  region            = var.region
  instance_name     = "streamhub-pg-${local.env}"
  network_id        = module.network.network_id
  psa_connection_id = module.network.psa_connection

  databases  = ["streamhub_prod", "streamhub_staging"]
  user_names = ["streamhub"]
  user_passwords = {
    streamhub = var.cloudsql_user_password
  }

  deletion_protection = true
}

module "redis" {
  source = "../../modules/redis"

  project_id        = var.project_id
  region            = var.region
  instance_id       = "streamhub-redis-${local.env}"
  network_id        = module.network.network_id
  psa_connection_id = module.network.psa_connection
  tier              = "STANDARD_HA"
  memory_size_gb    = 2
}

module "gcs" {
  source = "../../modules/gcs"

  project_id = var.project_id

  buckets = {
    recordings = {
      name               = "streamhub-recordings-${local.env}"
      lifecycle_age_days = 30
      lifecycle_action   = "Delete"
    }
    vod = {
      name = "streamhub-vod-${local.env}"
    }
    thumbnails = {
      name = "streamhub-thumbnails-${local.env}"
    }
  }
}

module "artifact_registry" {
  source = "../../modules/artifact-registry"

  project_id    = var.project_id
  location      = var.region
  repository_id = "streamhub"
}

resource "google_compute_global_address" "gateway_public" {
  project = var.project_id
  name    = "streamhub-public"
}

# Certificate Manager certs + map; the Gateway manifest references `streamhub-cert-map`
# via the `networking.gke.io/certmap` annotation and terminates TLS from it.
module "certs" {
  source = "../../modules/certs"

  project_id    = var.project_id
  cert_map_name = "streamhub-cert-map"

  certificates = {
    api = {
      certificate_name = "streamhub-api-cert"
      hostname         = var.domain_prod
    }
    admin = {
      certificate_name = "streamhub-admin-cert"
      hostname         = var.admin_domain_prod
    }
    web = {
      certificate_name = "streamhub-web-cert"
      hostname         = var.web_domain_prod
    }
  }
}

# Cloud Armor policies must exist before the GCPBackendPolicy manifests reference them
# by name; otherwise the backend service attachment fails during reconciliation.
module "cloud_armor_public" {
  source = "../../modules/cloud-armor"

  project_id  = var.project_id
  name        = "streamhub-public-armor"
  description = "Cloud Armor policy for api.streamhub.com and web.streamhub.com"

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
  name        = "streamhub-admin-armor"
  description = "Cloud Armor policy for admin.streamhub.com (stricter than public)"

  # Admin surface gets a tighter rate limit even without an IP allowlist.
  enable_rate_based_ban    = true
  rate_limit_count         = 30
  rate_limit_interval_sec  = 60
  rate_ban_duration_sec    = 1800
  enable_waf_preconfigured = true

  # Parameter wired up now; keep allowlist empty until operators add office / VPN CIDRs.
  enable_ip_allowlist = var.admin_ip_allowlist_enabled
  ip_allowlist        = var.admin_ip_allowlist
}

resource "helm_release" "external_secrets" {
  name             = "external-secrets"
  namespace        = "external-secrets"
  repository       = "https://charts.external-secrets.io"
  chart            = "external-secrets"
  version          = "2.3.0"
  create_namespace = true

  set {
    name  = "installCRDs"
    value = "true"
  }

  set {
    name  = "serviceAccount.create"
    value = "true"
  }

  set {
    name  = "serviceAccount.name"
    value = "external-secrets"
  }

  set {
    name  = "serviceAccount.annotations.iam\\.gke\\.io/gcp-service-account"
    value = google_service_account.eso.email
  }

  depends_on = [
    module.cluster,
    google_service_account_iam_member.eso_workload_identity,
    google_project_iam_member.eso,
  ]
}

output "gke_cluster_name" {
  value = module.cluster.cluster_name
}

output "gke_cluster_location" {
  value = module.cluster.cluster_location
}

output "workload_pool" {
  value = module.cluster.workload_pool
}

output "cloudsql_connection_name" {
  value = module.database.connection_name
}

output "cloudsql_private_ip" {
  value = module.database.private_ip_address
}

output "redis_host" {
  value = module.redis.host
}

output "redis_port" {
  value = module.redis.port
}

output "artifact_registry_image_prefix" {
  value = module.artifact_registry.image_prefix
}

output "gcs_buckets" {
  value = module.gcs.bucket_names
}

output "cloud_armor_public_name" {
  value       = module.cloud_armor_public.name
  description = "Name of the public Cloud Armor policy (referenced by GCPBackendPolicy)."
}

output "cloud_armor_admin_name" {
  value       = module.cloud_armor_admin.name
  description = "Name of the admin Cloud Armor policy (referenced by GCPBackendPolicy)."
}

output "certificate_map_name" {
  value       = module.certs.certificate_map_name
  description = "Certificate Manager map name (matches the Gateway `networking.gke.io/certmap` annotation)."
}

output "gateway_public_address_name" {
  value       = google_compute_global_address.gateway_public.name
  description = "Global static address resource name bound by the prod Gateway annotation."
}
