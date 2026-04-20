terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

locals {
  # Priority layout — earlier numbers are evaluated first.
  # 1000..1099 allowlist (optional)
  # 1100..1199 rate-based ban
  # 1200..1299 WAF preconfigured
  # 2147483647 default rule (fixed, Google-managed)

  allowlist_rule_enabled = var.enable_ip_allowlist && length(var.ip_allowlist) > 0
}

resource "google_compute_security_policy" "this" {
  project     = var.project_id
  name        = var.name
  description = var.description
  type        = "CLOUD_ARMOR"

  advanced_options_config {
    log_level = var.log_level
  }

  # Default rule (required). Behaviour depends on whether an IP allowlist is active:
  # - allowlist active  → default deny; only allowlisted ranges pass via the explicit allow rule.
  # - allowlist off     → default allow; specific rules below deny abuse / WAF hits.
  rule {
    action   = local.allowlist_rule_enabled ? "deny(403)" : "allow"
    priority = 2147483647
    match {
      versioned_expr = "SRC_IPS_V1"
      config {
        src_ip_ranges = ["*"]
      }
    }
    description = local.allowlist_rule_enabled ? "Default deny (allowlist enforced)" : "Default allow"
  }

  # Explicit allowlist — only attached when enabled AND the list is non-empty,
  # so the policy never ships with a deny-everything posture by accident.
  dynamic "rule" {
    for_each = local.allowlist_rule_enabled ? [1] : []
    content {
      action   = "allow"
      priority = 1000
      match {
        versioned_expr = "SRC_IPS_V1"
        config {
          src_ip_ranges = var.ip_allowlist
        }
      }
      description = "IP allowlist"
    }
  }

  # Rate-based ban: abuse shield for public endpoints. Keyed by source IP.
  dynamic "rule" {
    for_each = var.enable_rate_based_ban ? [1] : []
    content {
      action   = "rate_based_ban"
      priority = 1100
      match {
        versioned_expr = "SRC_IPS_V1"
        config {
          src_ip_ranges = ["*"]
        }
      }
      rate_limit_options {
        conform_action = "allow"
        exceed_action  = "deny(429)"
        enforce_on_key = "IP"
        rate_limit_threshold {
          count        = var.rate_limit_count
          interval_sec = var.rate_limit_interval_sec
        }
        ban_duration_sec = var.rate_ban_duration_sec
      }
      description = "Rate-based ban per source IP"
    }
  }

  # Preconfigured WAF: SQLi sensitivity 1.
  dynamic "rule" {
    for_each = var.enable_waf_preconfigured ? [1] : []
    content {
      action   = "deny(403)"
      priority = 1200
      match {
        expr {
          expression = "evaluatePreconfiguredExpr('sqli-v33-stable', ['owasp-crs-v030301-id942110-sqli', 'owasp-crs-v030301-id942120-sqli'])"
        }
      }
      description = "Block SQLi (preconfigured WAF, sensitivity 1)"
    }
  }

  # Preconfigured WAF: XSS sensitivity 1.
  dynamic "rule" {
    for_each = var.enable_waf_preconfigured ? [1] : []
    content {
      action   = "deny(403)"
      priority = 1201
      match {
        expr {
          expression = "evaluatePreconfiguredExpr('xss-v33-stable', ['owasp-crs-v030301-id941110-xss', 'owasp-crs-v030301-id941120-xss'])"
        }
      }
      description = "Block XSS (preconfigured WAF, sensitivity 1)"
    }
  }
}
