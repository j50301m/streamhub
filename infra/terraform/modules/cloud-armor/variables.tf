variable "project_id" {
  description = "GCP project ID."
  type        = string
}

variable "name" {
  description = "Name of the Cloud Armor security policy."
  type        = string
}

variable "description" {
  description = "Human-readable description."
  type        = string
  default     = ""
}

variable "enable_rate_based_ban" {
  description = "Whether to add a rate-based-ban rule (abuse shield). Typically on for public, off for admin with allowlist."
  type        = bool
  default     = true
}

variable "rate_limit_count" {
  description = "Requests per interval that triggers the rate-based ban."
  type        = number
  default     = 100
}

variable "rate_limit_interval_sec" {
  description = "Interval (seconds) over which `rate_limit_count` is measured."
  type        = number
  default     = 60
}

variable "rate_ban_duration_sec" {
  description = "Ban duration (seconds) once the rate-based threshold is exceeded."
  type        = number
  default     = 600
}

variable "enable_waf_preconfigured" {
  description = "Attach Google-managed preconfigured WAF rules (SQLi + XSS sensitivity 1)."
  type        = bool
  default     = true
}

variable "enable_ip_allowlist" {
  description = "If true, reject every request whose source IP is not in `ip_allowlist`. Useful to lock admin traffic to office / VPN ranges."
  type        = bool
  default     = false
}

variable "ip_allowlist" {
  description = "Source CIDRs allowed when `enable_ip_allowlist` is true. Ignored otherwise."
  type        = list(string)
  default     = []
}

variable "log_level" {
  description = "Cloud Armor log level: NORMAL or VERBOSE."
  type        = string
  default     = "NORMAL"
}
