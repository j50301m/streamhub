terraform {
  required_version = ">= 1.9.0"

  required_providers {
    google = {
      source  = "hashicorp/google"
      version = "~> 6.12"
    }
    google-beta = {
      source  = "hashicorp/google-beta"
      version = "~> 6.12"
    }
  }
}

resource "google_container_cluster" "primary" {
  provider = google-beta

  name     = var.cluster_name
  project  = var.project_id
  location = var.region

  node_locations = var.node_locations

  release_channel {
    channel = var.release_channel
  }

  # Remove the default pool; we manage our own pools below.
  remove_default_node_pool = true
  initial_node_count       = 1

  network    = var.network_self_link
  subnetwork = var.subnet_self_link

  networking_mode = "VPC_NATIVE"

  ip_allocation_policy {
    cluster_secondary_range_name  = var.pods_range_name
    services_secondary_range_name = var.services_range_name
  }

  private_cluster_config {
    enable_private_nodes    = true
    enable_private_endpoint = false
    master_ipv4_cidr_block  = var.master_ipv4_cidr

    master_global_access_config {
      enabled = true
    }
  }

  master_authorized_networks_config {
    dynamic "cidr_blocks" {
      for_each = var.master_authorized_networks
      content {
        cidr_block   = cidr_blocks.value.cidr_block
        display_name = cidr_blocks.value.display_name
      }
    }
  }

  workload_identity_config {
    workload_pool = "${var.project_id}.svc.id.goog"
  }

  addons_config {
    http_load_balancing {
      disabled = false
    }
    horizontal_pod_autoscaling {
      disabled = false
    }
    network_policy_config {
      disabled = false
    }
    gcs_fuse_csi_driver_config {
      enabled = true
    }
    gce_persistent_disk_csi_driver_config {
      enabled = true
    }
    gke_backup_agent_config {
      enabled = true
    }
  }

  # Enable Gateway API (gateway.networking.k8s.io/v1).
  gateway_api_config {
    channel = "CHANNEL_STANDARD"
  }

  # Enable managed Prometheus so SPEC-011c can route metrics via GMP.
  monitoring_config {
    enable_components = [
      "SYSTEM_COMPONENTS",
      "APISERVER",
      "CONTROLLER_MANAGER",
      "SCHEDULER",
      "WORKLOADS",
    ]

    managed_prometheus {
      enabled = true
    }
  }

  logging_config {
    enable_components = [
      "SYSTEM_COMPONENTS",
      "WORKLOADS",
      "APISERVER",
    ]
  }

  network_policy {
    enabled  = true
    provider = "CALICO"
  }

  # Deletion protection off to allow IaC teardown in non-prod; flip true in prod overlay.
  deletion_protection = false

  lifecycle {
    ignore_changes = [
      # Node pool list is managed below; ignore drift caused by removing default pool.
      initial_node_count,
      node_config,
    ]
  }
}

locals {
  default_oauth_scopes = [
    "https://www.googleapis.com/auth/cloud-platform",
  ]
}

resource "google_container_node_pool" "app" {
  name     = "app"
  project  = var.project_id
  location = var.region
  cluster  = google_container_cluster.primary.name

  autoscaling {
    min_node_count = var.app_pool_min_nodes_per_zone
    max_node_count = var.app_pool_max_nodes_per_zone
  }

  management {
    auto_repair  = true
    auto_upgrade = true
  }

  upgrade_settings {
    max_surge       = 1
    max_unavailable = 0
    strategy        = "SURGE"
  }

  node_config {
    machine_type = var.app_pool_machine_type
    disk_size_gb = 100
    disk_type    = "pd-balanced"
    image_type   = "COS_CONTAINERD"

    oauth_scopes = local.default_oauth_scopes

    labels = {
      pool = "app"
    }

    workload_metadata_config {
      mode = "GKE_METADATA"
    }

    shielded_instance_config {
      enable_secure_boot          = true
      enable_integrity_monitoring = true
    }
  }
}

resource "google_container_node_pool" "media" {
  name     = "media"
  project  = var.project_id
  location = var.region
  cluster  = google_container_cluster.primary.name

  autoscaling {
    min_node_count = var.media_pool_min_nodes_per_zone
    max_node_count = var.media_pool_max_nodes_per_zone
  }

  management {
    auto_repair  = true
    auto_upgrade = true
  }

  upgrade_settings {
    max_surge       = 1
    max_unavailable = 0
    strategy        = "SURGE"
  }

  node_config {
    machine_type = var.media_pool_machine_type
    disk_size_gb = 200
    disk_type    = "pd-balanced"
    image_type   = "COS_CONTAINERD"

    oauth_scopes = local.default_oauth_scopes

    # Only MediaMTX pods should land here (SPEC-011b attaches a matching toleration).
    taint {
      key    = "workload"
      value  = "media"
      effect = "NO_SCHEDULE"
    }

    labels = {
      pool = "media"
    }

    workload_metadata_config {
      mode = "GKE_METADATA"
    }

    shielded_instance_config {
      enable_secure_boot          = true
      enable_integrity_monitoring = true
    }
  }
}

resource "google_container_node_pool" "ops" {
  name     = "ops"
  project  = var.project_id
  location = var.region
  cluster  = google_container_cluster.primary.name

  autoscaling {
    min_node_count = var.ops_pool_min_nodes_per_zone
    max_node_count = var.ops_pool_max_nodes_per_zone
  }

  management {
    auto_repair  = true
    auto_upgrade = true
  }

  upgrade_settings {
    max_surge       = 1
    max_unavailable = 0
    strategy        = "SURGE"
  }

  node_config {
    machine_type = var.ops_pool_machine_type
    disk_size_gb = 100
    disk_type    = "pd-balanced"
    image_type   = "COS_CONTAINERD"

    oauth_scopes = local.default_oauth_scopes

    taint {
      key    = "workload"
      value  = "ops"
      effect = "NO_SCHEDULE"
    }

    labels = {
      pool = "ops"
    }

    workload_metadata_config {
      mode = "GKE_METADATA"
    }

    shielded_instance_config {
      enable_secure_boot          = true
      enable_integrity_monitoring = true
    }
  }
}
