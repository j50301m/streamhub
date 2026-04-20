output "certificate_map_name" {
  value       = google_certificate_manager_certificate_map.this.name
  description = "Name of the certificate map; matches the networking.gke.io/certmap annotation on the Gateway."
}

output "certificate_ids" {
  value       = { for k, c in google_certificate_manager_certificate.this : k => c.id }
  description = "Map of logical key -> certificate resource ID."
}
