output "cluster_name" {
  value       = google_container_cluster.primary.name
  description = "Name of the GKE cluster."
}

output "cluster_location" {
  value       = google_container_cluster.primary.location
  description = "Location (region) of the GKE cluster."
}

output "cluster_endpoint" {
  value       = google_container_cluster.primary.endpoint
  description = "API server endpoint."
  sensitive   = true
}

output "cluster_ca_certificate" {
  value       = google_container_cluster.primary.master_auth[0].cluster_ca_certificate
  description = "Base64-encoded cluster CA certificate."
  sensitive   = true
}

output "workload_pool" {
  value       = google_container_cluster.primary.workload_identity_config[0].workload_pool
  description = "Workload Identity pool for KSA<->GSA bindings."
}
