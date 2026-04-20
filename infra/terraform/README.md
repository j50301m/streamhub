# streamhub — Terraform

Terraform roots live under `envs/`:

```
envs/
├── prod/      # provisions VPC, GKE regional cluster, Cloud SQL, Memorystore, GCS, Artifact Registry
└── staging/   # provisions staging-only GCS buckets (staging workloads share the prod cluster via namespaces)
```

Reusable building blocks live under `modules/`:

```
modules/
├── network/            # VPC + subnet + PSA (Cloud SQL / Memorystore) + Cloud NAT
├── cluster/            # GKE regional cluster + app/media/ops node pools + Gateway API + Workload Identity
├── database/           # Cloud SQL Postgres 17 (private IP only) + databases + users
├── redis/              # Memorystore Redis 7 on PSA, auth + in-transit encryption
├── gcs/                # Buckets (recordings / vod / thumbnails) with lifecycle rules
├── artifact-registry/  # Single DOCKER repo for api/bo-api/web/mediamtx images
└── cloud-armor/        # Cloud Armor SecurityPolicy (rate-based ban + preconfigured WAF + optional IP allowlist)
```

**Cloud Armor policies are fully Terraform-managed.** Do not edit them with
`gcloud compute security-policies` — any manual change will be reverted on the
next `terraform apply`. The GKE `GCPBackendPolicy` manifests in
`deploy/gke/base/gateway/` reference these policies by name, so the Terraform
apply must land before the Kustomize apply.

## Prerequisites

- Terraform `>= 1.9`
- `gcloud auth application-default login`
- A GCS bucket for Terraform state (create manually once per project)

## First apply (prod root)

```bash
cd infra/terraform/envs/prod

# Point backend at the state bucket
terraform init \
  -backend-config="bucket=streamhub-tfstate-prod" \
  -backend-config="prefix=envs/prod"

# Fill variables
cp terraform.tfvars.example terraform.tfvars
$EDITOR terraform.tfvars

terraform fmt -check
terraform validate
terraform plan
terraform apply
```

Staging root lives at `envs/staging` and only creates staging-specific resources
(GCS buckets and two `-staging`-suffixed Cloud Armor policies) — the GKE cluster,
Cloud SQL instance, Memorystore, and Artifact Registry are shared with prod and
provisioned by `envs/prod`. The staging Kustomize overlay at
`deploy/gke/staging/` patches the `GCPBackendPolicy` names to point at the
`-staging`-suffixed policies so prod and staging can coexist in the same project.

## After apply

1. Fetch cluster credentials:

   ```bash
   gcloud container clusters get-credentials streamhub-prod \
     --region asia-east1 --project <project-id>
   ```

2. Push Kustomize manifests from `deploy/gke/`:

   ```bash
   deploy/gke/scripts/deploy.sh prod <image-tag>
   ```

## Secrets

No service-account JSON keys are stored here. Workload Identity handles KSA → GSA
impersonation; application secrets (DB password, JWT secret, Redis auth) live in
Secret Manager and are surfaced to pods via External Secrets Operator (see
`deploy/gke/base/eso/`).
