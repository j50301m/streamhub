output "repository_id" {
  value       = google_artifact_registry_repository.images.repository_id
  description = "Repository ID."
}

output "repository_name" {
  value       = google_artifact_registry_repository.images.name
  description = "Fully qualified repository resource name."
}

output "image_prefix" {
  value       = "${var.location}-docker.pkg.dev/${var.project_id}/${google_artifact_registry_repository.images.repository_id}"
  description = "Prefix for pushing images (append /<image>:<tag>)."
}
