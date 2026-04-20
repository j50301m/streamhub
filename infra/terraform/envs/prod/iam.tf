################################################################################
# Workload Identity: GSA per app KSA + IAM roles.
#
# The KSA namespaces referenced here must match the Kustomize base (`streamhub-prod`,
# `streamhub-staging`). Helm/operator-managed ESO runs in the `external-secrets`
# namespace.
################################################################################

locals {
  workload_pool = module.cluster.workload_pool

  api_namespaces    = ["streamhub-prod", "streamhub-staging"]
  bo_api_namespaces = ["streamhub-prod", "streamhub-staging"]

  api_gsa_roles = [
    "roles/cloudtrace.agent",
    "roles/monitoring.metricWriter",
    "roles/logging.logWriter",
    "roles/pubsub.publisher",
    "roles/pubsub.subscriber",
    "roles/transcoder.admin",
    "roles/secretmanager.secretAccessor",
  ]

  bo_api_gsa_roles = [
    "roles/cloudtrace.agent",
    "roles/monitoring.metricWriter",
    "roles/logging.logWriter",
    "roles/secretmanager.secretAccessor",
  ]

  eso_gsa_roles = [
    "roles/secretmanager.secretAccessor",
  ]
}

# ---------- api ----------

resource "google_service_account" "api" {
  project      = var.project_id
  account_id   = "streamhub-api"
  display_name = "streamhub api (Workload Identity)"
}

resource "google_project_iam_member" "api" {
  for_each = toset(local.api_gsa_roles)

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.api.email}"
}

resource "google_storage_bucket_iam_member" "api_recordings" {
  bucket = module.gcs.bucket_names["recordings"]
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.api.email}"
}

resource "google_storage_bucket_iam_member" "api_vod" {
  bucket = module.gcs.bucket_names["vod"]
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.api.email}"
}

resource "google_storage_bucket_iam_member" "api_thumbnails" {
  bucket = module.gcs.bucket_names["thumbnails"]
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.api.email}"
}

resource "google_service_account_iam_member" "api_workload_identity" {
  for_each = toset(local.api_namespaces)

  service_account_id = google_service_account.api.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${local.workload_pool}[${each.value}/api]"
}

# ---------- bo-api ----------

resource "google_service_account" "bo_api" {
  project      = var.project_id
  account_id   = "streamhub-bo-api"
  display_name = "streamhub bo-api (Workload Identity)"
}

resource "google_project_iam_member" "bo_api" {
  for_each = toset(local.bo_api_gsa_roles)

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.bo_api.email}"
}

resource "google_storage_bucket_iam_member" "bo_api_recordings_read" {
  bucket = module.gcs.bucket_names["recordings"]
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.bo_api.email}"
}

resource "google_storage_bucket_iam_member" "bo_api_vod_read" {
  bucket = module.gcs.bucket_names["vod"]
  role   = "roles/storage.objectViewer"
  member = "serviceAccount:${google_service_account.bo_api.email}"
}

resource "google_service_account_iam_member" "bo_api_workload_identity" {
  for_each = toset(local.bo_api_namespaces)

  service_account_id = google_service_account.bo_api.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${local.workload_pool}[${each.value}/bo-api]"
}

# ---------- External Secrets Operator ----------

resource "google_service_account" "eso" {
  project      = var.project_id
  account_id   = "streamhub-eso"
  display_name = "External Secrets Operator (Workload Identity)"
}

resource "google_project_iam_member" "eso" {
  for_each = toset(local.eso_gsa_roles)

  project = var.project_id
  role    = each.value
  member  = "serviceAccount:${google_service_account.eso.email}"
}

resource "google_service_account_iam_member" "eso_workload_identity" {
  service_account_id = google_service_account.eso.name
  role               = "roles/iam.workloadIdentityUser"
  member             = "serviceAccount:${local.workload_pool}[external-secrets/external-secrets]"
}

output "gsa_api_email" {
  value       = google_service_account.api.email
  description = "Email of the api GSA; put this in the KSA `iam.gke.io/gcp-service-account` annotation."
}

output "gsa_bo_api_email" {
  value       = google_service_account.bo_api.email
  description = "Email of the bo-api GSA."
}

output "gsa_eso_email" {
  value       = google_service_account.eso.email
  description = "Email of the ESO GSA."
}
