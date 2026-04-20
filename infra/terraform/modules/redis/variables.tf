variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "region" {
  description = "Memorystore region."
  type        = string
  default     = "asia-east1"
}

variable "instance_id" {
  description = "Memorystore instance ID."
  type        = string
  default     = "streamhub-redis"
}

variable "tier" {
  description = "Memorystore tier (BASIC or STANDARD_HA)."
  type        = string
  default     = "STANDARD_HA"
}

variable "memory_size_gb" {
  description = "Redis memory capacity in GB."
  type        = number
  default     = 2
}

variable "redis_version" {
  description = "Redis engine version."
  type        = string
  default     = "REDIS_7_2"
}

variable "network_id" {
  description = "VPC network ID (PSA attached)."
  type        = string
}

variable "psa_connection_id" {
  description = "PSA peering connection ID; forces Memorystore to wait for peering."
  type        = string
}

variable "connect_mode" {
  description = "PRIVATE_SERVICE_ACCESS (PSA) or DIRECT_PEERING."
  type        = string
  default     = "PRIVATE_SERVICE_ACCESS"
}

variable "auth_enabled" {
  description = "Enable Redis AUTH."
  type        = bool
  default     = true
}

variable "transit_encryption_mode" {
  description = "Transit encryption mode."
  type        = string
  default     = "SERVER_AUTHENTICATION"
}
