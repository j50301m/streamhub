variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "location" {
  description = "Bucket location (asia-east1 for Taiwan)."
  type        = string
  default     = "ASIA-EAST1"
}

variable "buckets" {
  description = "Map of logical bucket name to bucket spec."
  type = map(object({
    name                        = string
    uniform_bucket_level_access = optional(bool, true)
    force_destroy               = optional(bool, false)
    versioning                  = optional(bool, false)
    lifecycle_age_days          = optional(number, null)
    lifecycle_action            = optional(string, "Delete")
  }))
}
