variable "project_id" {
  description = "GCP project ID for streamhub prod."
  type        = string
}

variable "region" {
  description = "Primary region."
  type        = string
  default     = "asia-east1"
}

variable "master_authorized_networks" {
  description = "CIDRs allowed to reach the GKE control plane."
  type = list(object({
    cidr_block   = string
    display_name = string
  }))
  default = []
}

variable "cloudsql_user_password" {
  description = "Password for the initial streamhub Cloud SQL user. Populate via -var-file or Secret Manager pipeline."
  type        = string
  sensitive   = true
}

variable "domain_prod" {
  description = "Public API domain for prod workloads."
  type        = string
  default     = "api.streamhub.com"
}

variable "admin_domain_prod" {
  description = "Admin API domain for prod workloads."
  type        = string
  default     = "admin.streamhub.com"
}

variable "web_domain_prod" {
  description = "Web frontend domain for prod workloads."
  type        = string
  default     = "web.streamhub.com"
}

variable "admin_ip_allowlist_enabled" {
  description = "If true, restrict admin.streamhub.com to CIDRs in admin_ip_allowlist. Leave false until operators confirm office / VPN ranges."
  type        = bool
  default     = false
}

variable "admin_ip_allowlist" {
  description = "Source CIDRs allowed to hit admin.streamhub.com when admin_ip_allowlist_enabled is true."
  type        = list(string)
  default     = []
}
