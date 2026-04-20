output "bucket_names" {
  value       = { for k, b in google_storage_bucket.this : k => b.name }
  description = "Map of logical bucket key to real bucket name."
}

output "bucket_urls" {
  value       = { for k, b in google_storage_bucket.this : k => b.url }
  description = "gs:// URLs keyed by logical name."
}
