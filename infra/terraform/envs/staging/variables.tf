variable "project_id" {
  description = "GCP project ID for streamhub staging (often the same as prod since we share the cluster)."
  type        = string
}

variable "region" {
  description = "Primary region."
  type        = string
  default     = "asia-east1"
}

variable "domain_staging" {
  description = "Public API domain for staging workloads."
  type        = string
  default     = "api.staging.streamhub.com"
}

variable "admin_domain_staging" {
  description = "Admin API domain for staging workloads."
  type        = string
  default     = "admin.staging.streamhub.com"
}

variable "web_domain_staging" {
  description = "Web frontend domain for staging workloads."
  type        = string
  default     = "web.staging.streamhub.com"
}
