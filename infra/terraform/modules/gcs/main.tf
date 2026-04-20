terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

resource "google_storage_bucket" "this" {
  for_each = var.buckets

  project                     = var.project_id
  name                        = each.value.name
  location                    = var.location
  force_destroy               = each.value.force_destroy
  uniform_bucket_level_access = each.value.uniform_bucket_level_access
  public_access_prevention    = "enforced"

  versioning {
    enabled = each.value.versioning
  }

  dynamic "lifecycle_rule" {
    for_each = each.value.lifecycle_age_days == null ? [] : [each.value.lifecycle_age_days]
    content {
      action {
        type = each.value.lifecycle_action
      }
      condition {
        age = lifecycle_rule.value
      }
    }
  }
}
