terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

resource "google_certificate_manager_certificate" "this" {
  for_each = var.certificates

  project  = var.project_id
  name     = each.value.certificate_name
  location = "global"

  managed {
    domains = [each.value.hostname]
  }
}

resource "google_certificate_manager_certificate_map" "this" {
  project = var.project_id
  name    = var.cert_map_name
}

resource "google_certificate_manager_certificate_map_entry" "this" {
  for_each = var.certificates

  project      = var.project_id
  name         = "${each.value.certificate_name}-entry"
  map          = google_certificate_manager_certificate_map.this.name
  certificates = [google_certificate_manager_certificate.this[each.key].id]
  hostname     = each.value.hostname
}
