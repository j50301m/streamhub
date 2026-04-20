output "network_id" {
  value       = google_compute_network.vpc.id
  description = "Full ID of the VPC network."
}

output "network_name" {
  value       = google_compute_network.vpc.name
  description = "Name of the VPC network."
}

output "network_self_link" {
  value       = google_compute_network.vpc.self_link
  description = "Self link of the VPC network."
}

output "subnet_id" {
  value       = google_compute_subnetwork.gke.id
  description = "Full ID of the GKE subnet."
}

output "subnet_self_link" {
  value       = google_compute_subnetwork.gke.self_link
  description = "Self link of the GKE subnet."
}

output "pods_range_name" {
  value       = var.pods_range_name
  description = "Secondary range name for pods."
}

output "services_range_name" {
  value       = var.services_range_name
  description = "Secondary range name for services."
}

output "master_ipv4_cidr" {
  value       = var.master_ipv4_cidr
  description = "Control plane CIDR (consumed by the cluster module)."
}

output "psa_connection" {
  value       = google_service_networking_connection.psa.id
  description = "PSA peering connection ID; downstream modules depend on this to ensure peering is up."
}
