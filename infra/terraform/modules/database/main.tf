terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
  }
}

resource "google_sql_database_instance" "primary" {
  name                = var.instance_name
  project             = var.project_id
  region              = var.region
  database_version    = var.database_version
  deletion_protection = var.deletion_protection

  # Cloud SQL creation must wait for PSA peering; otherwise private IP allocation fails.
  depends_on = [var.psa_connection_id]

  settings {
    tier              = var.tier
    availability_type = var.availability_type
    disk_size         = var.disk_size_gb
    disk_type         = var.disk_type
    disk_autoresize   = true

    ip_configuration {
      ipv4_enabled                                  = false
      private_network                               = var.network_id
      enable_private_path_for_google_cloud_services = true
    }

    backup_configuration {
      enabled                        = true
      start_time                     = "03:00"
      point_in_time_recovery_enabled = true
      transaction_log_retention_days = 7

      backup_retention_settings {
        retained_backups = 14
        retention_unit   = "COUNT"
      }
    }

    maintenance_window {
      day          = 7
      hour         = 4
      update_track = "stable"
    }

    insights_config {
      query_insights_enabled  = true
      record_application_tags = false
      record_client_address   = false
    }

    # IAM auth left off for now: users are password-authenticated via Secret Manager.
    # Switching to IAM auth is a follow-up (different KSA wiring).
  }
}

resource "google_sql_database" "app" {
  for_each = toset(var.databases)

  name     = each.value
  project  = var.project_id
  instance = google_sql_database_instance.primary.name
}

resource "google_sql_user" "app" {
  # `user_names` is the non-sensitive driver for `for_each`; the password is
  # looked up from the sensitive map `user_passwords` inside the body, which
  # Terraform allows.
  for_each = toset(var.user_names)

  name     = each.key
  project  = var.project_id
  instance = google_sql_database_instance.primary.name
  password = var.user_passwords[each.key]
}
