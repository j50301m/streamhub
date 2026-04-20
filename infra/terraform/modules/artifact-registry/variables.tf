variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "location" {
  description = "Artifact Registry location."
  type        = string
  default     = "asia-east1"
}

variable "repository_id" {
  description = "Repository ID (Docker images for api/bo-api/web/mediamtx)."
  type        = string
  default     = "streamhub"
}

variable "description" {
  description = "Human-readable description."
  type        = string
  default     = "streamhub container images"
}

variable "cleanup_keep_recent_versions" {
  description = "Keep this many recent versions per image (cleanup policy)."
  type        = number
  default     = 20
}

variable "cleanup_delete_older_than_days" {
  description = "Delete untagged versions older than this many days."
  type        = number
  default     = 30
}
