variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "region" {
  description = "Cloud SQL region."
  type        = string
  default     = "asia-east1"
}

variable "instance_name" {
  description = "Cloud SQL instance name."
  type        = string
  default     = "streamhub-pg"
}

variable "database_version" {
  description = "Cloud SQL Postgres version."
  type        = string
  default     = "POSTGRES_17"
}

variable "tier" {
  description = "Machine tier for the primary instance."
  type        = string
  default     = "db-custom-2-7680"
}

variable "availability_type" {
  description = "Availability type: REGIONAL (HA) or ZONAL."
  type        = string
  default     = "REGIONAL"
}

variable "disk_size_gb" {
  description = "Initial disk size in GB."
  type        = number
  default     = 50
}

variable "disk_type" {
  description = "Disk type."
  type        = string
  default     = "PD_SSD"
}

variable "network_id" {
  description = "VPC network ID (PSA peering is attached to this)."
  type        = string
}

variable "psa_connection_id" {
  description = "PSA peering connection ID from the network module (used as explicit dependency)."
  type        = string
}

variable "databases" {
  description = "Databases to create inside the instance (one per env namespace)."
  type        = list(string)
  default     = ["streamhub_prod", "streamhub_staging"]
}

variable "user_names" {
  description = "Names of Cloud SQL users to provision. Non-sensitive so it can be used as `for_each`."
  type        = list(string)
  default     = []
}

variable "user_passwords" {
  description = "Map of user name -> password. Keys must exactly match `user_names`. Populated from Secret Manager in real rollouts."
  type        = map(string)
  default     = {}
  sensitive   = true
}

variable "deletion_protection" {
  description = "Whether Cloud SQL instance is deletion-protected."
  type        = bool
  default     = true
}
