output "host" {
  value       = google_redis_instance.primary.host
  description = "Primary Redis host (private IP)."
}

output "port" {
  value       = google_redis_instance.primary.port
  description = "Redis port."
}

output "auth_string" {
  value       = google_redis_instance.primary.auth_string
  description = "Redis AUTH string; write to Secret Manager instead of hard-coding."
  sensitive   = true
}

output "current_location_id" {
  value       = google_redis_instance.primary.current_location_id
  description = "Primary zone (for debugging)."
}

output "instance_id" {
  value       = google_redis_instance.primary.id
  description = "Full Memorystore instance ID."
}
