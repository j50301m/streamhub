output "name" {
  value       = google_compute_security_policy.this.name
  description = "Security policy name (referenced by GCPBackendPolicy.spec.default.securityPolicy)."
}

output "self_link" {
  value       = google_compute_security_policy.this.self_link
  description = "Fully qualified self link."
}

output "id" {
  value       = google_compute_security_policy.this.id
  description = "Resource ID."
}
