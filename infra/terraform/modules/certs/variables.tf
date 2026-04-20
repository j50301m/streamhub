variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "cert_map_name" {
  description = "Name of the certificate map referenced by the Gateway's networking.gke.io/certmap annotation."
  type        = string
  default     = "streamhub-cert-map"
}

variable "certificates" {
  description = "Map of logical key -> certificate spec. Each entry provisions one Google-managed certificate plus a cert map entry for the given hostname."
  type = map(object({
    # Name of the google_certificate_manager_certificate resource (globally unique per project).
    certificate_name = string
    # Fully qualified DNS name covered by the certificate (e.g. api.streamhub.com).
    hostname = string
  }))
}
