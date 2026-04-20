variable "project_id" {
  description = "GCP project ID hosting the VPC."
  type        = string
}

variable "region" {
  description = "Primary region for subnets."
  type        = string
  default     = "asia-east1"
}

variable "network_name" {
  description = "Name of the VPC network."
  type        = string
  default     = "streamhub-vpc"
}

variable "subnet_name" {
  description = "Name of the primary GKE subnet."
  type        = string
  default     = "streamhub-gke-subnet"
}

variable "subnet_cidr" {
  description = "Primary CIDR for the GKE node subnet."
  type        = string
  default     = "10.10.0.0/20"
}

variable "pods_range_name" {
  description = "Secondary range name for GKE pods."
  type        = string
  default     = "streamhub-pods"
}

variable "pods_cidr" {
  description = "Secondary CIDR for GKE pods."
  type        = string
  default     = "10.20.0.0/14"
}

variable "services_range_name" {
  description = "Secondary range name for GKE services."
  type        = string
  default     = "streamhub-services"
}

variable "services_cidr" {
  description = "Secondary CIDR for GKE services."
  type        = string
  default     = "10.24.0.0/20"
}

variable "master_ipv4_cidr" {
  description = "CIDR for the GKE control plane (private cluster peering range)."
  type        = string
  default     = "172.16.0.0/28"
}

variable "psa_range_name" {
  description = "Name of the Private Service Access allocated range (used by Cloud SQL + Memorystore)."
  type        = string
  default     = "streamhub-psa"
}

variable "psa_prefix_length" {
  description = "Prefix length of the PSA allocated range."
  type        = number
  default     = 16
}
