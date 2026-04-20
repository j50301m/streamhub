variable "project_id" {
  description = "GCP project ID hosting the GKE cluster."
  type        = string
}

variable "region" {
  description = "Region for the regional GKE cluster."
  type        = string
  default     = "asia-east1"
}

variable "node_locations" {
  description = "Zones inside the region used for node pools (regional cluster spreads nodes across these)."
  type        = list(string)
  default     = ["asia-east1-a", "asia-east1-b", "asia-east1-c"]
}

variable "cluster_name" {
  description = "Name of the GKE cluster."
  type        = string
  default     = "streamhub"
}

variable "release_channel" {
  description = "GKE release channel."
  type        = string
  default     = "REGULAR"
}

variable "network_self_link" {
  description = "VPC self link from the network module."
  type        = string
}

variable "subnet_self_link" {
  description = "Subnet self link from the network module."
  type        = string
}

variable "pods_range_name" {
  description = "Secondary range name for pods (from network module)."
  type        = string
}

variable "services_range_name" {
  description = "Secondary range name for services (from network module)."
  type        = string
}

variable "master_ipv4_cidr" {
  description = "Control plane CIDR for private cluster peering."
  type        = string
}

variable "master_authorized_networks" {
  description = "CIDRs allowed to talk to the GKE control plane (operators / CI)."
  type = list(object({
    cidr_block   = string
    display_name = string
  }))
  default = []
}

variable "app_pool_machine_type" {
  description = "Machine type for the app node pool (api/bo-api/web)."
  type        = string
  default     = "e2-standard-4"
}

variable "app_pool_min_nodes_per_zone" {
  description = "Minimum nodes per zone for the app pool (regional cluster multiplies by zones)."
  type        = number
  default     = 1
}

variable "app_pool_max_nodes_per_zone" {
  description = "Maximum nodes per zone for the app pool."
  type        = number
  default     = 4
}

variable "media_pool_machine_type" {
  description = "Machine type for the media node pool (reserved for MediaMTX in SPEC-011b)."
  type        = string
  default     = "n2-standard-4"
}

variable "media_pool_min_nodes_per_zone" {
  description = "Minimum nodes per zone for the media pool."
  type        = number
  default     = 0
}

variable "media_pool_max_nodes_per_zone" {
  description = "Maximum nodes per zone for the media pool."
  type        = number
  default     = 2
}

variable "ops_pool_machine_type" {
  description = "Machine type for the ops node pool (reserved for observability in SPEC-011c)."
  type        = string
  default     = "e2-standard-4"
}

variable "ops_pool_min_nodes_per_zone" {
  description = "Minimum nodes per zone for the ops pool."
  type        = number
  default     = 0
}

variable "ops_pool_max_nodes_per_zone" {
  description = "Maximum nodes per zone for the ops pool."
  type        = number
  default     = 2
}
