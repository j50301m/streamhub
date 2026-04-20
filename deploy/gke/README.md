# streamhub — GKE Kustomize manifests

```
deploy/gke/
├── base/
│   ├── namespaces/      # streamhub-prod, streamhub-staging, observability
│   ├── api/             # Deployment, Service, HPA, PDB, ServiceAccount, ConfigMap
│   ├── bo-api/          # same shape as api
│   ├── web/             # same shape as api (no Workload Identity)
│   ├── gateway/         # GKE Gateway API + Google-managed certs + HTTPRoutes + BackendPolicies
│   ├── networkpolicy/   # default-deny + allow-dns + per-app policies
│   └── eso/             # ClusterSecretStore + ExternalSecret definitions (ESO)
├── prod/                # overlay for streamhub-prod namespace
│   ├── kustomization.yaml
│   └── patches/         # SA annotations, ConfigMap overrides, ClusterSecretStore patch
├── staging/             # overlay for streamhub-staging namespace (smaller HPA)
│   └── patches/
└── scripts/
    ├── deploy.sh        # human entrypoint: validates context, pins image tag, applies overlay
    ├── rollback.sh      # `kubectl rollout undo` per deployment
    └── diff.sh          # `kubectl diff` against the live cluster
```

## Prerequisites on the operator box

- `gcloud` authenticated and pointed at the correct project
- `kubectl` authenticated against the right cluster (`gcloud container clusters get-credentials streamhub-prod --region asia-east1`)
- `kustomize` v5+ (`brew install kustomize`)
- `terraform apply` for `infra/terraform/envs/prod` already completed once for the shared cluster baseline
- `terraform apply` for `infra/terraform/envs/staging` completed before the first staging rollout if staging hostnames / certs are enabled

## Deploy

```bash
# staging
deploy/gke/scripts/deploy.sh staging "$(git rev-parse HEAD)"

# prod (after staging smoke — see SPEC-011d)
deploy/gke/scripts/deploy.sh prod "$(git rev-parse HEAD)"
```

The script:
1. verifies the active kube-context matches the expected cluster
2. rewrites `images:` entries in a temp copy of the overlay to point at
   `asia-east1-docker.pkg.dev/<project>/streamhub/<image>:<tag>`
3. runs `kustomize build | kubectl apply -f -`
4. waits for rollout of `api`, `bo-api`, `web`

## Rollback

```bash
deploy/gke/scripts/rollback.sh prod            # rolls back all three
deploy/gke/scripts/rollback.sh prod api        # rolls back just the api
```

`rollback.sh` uses `kubectl rollout undo`, which flips back to the previous
ReplicaSet. For targeted image rollback, re-run `deploy.sh prod <older-tag>`.

## Image tag convention

Images are tagged with the **full git SHA**. The deploy script will refuse to
run without `IMAGE_TAG` (or a second positional arg), so we never deploy a
floating tag like `latest`.

## Secrets

No plaintext secrets live in this directory. `base/eso/` holds ExternalSecret
objects that map Secret Manager entries to Kubernetes Secrets. Secret Manager
entries are created out-of-band (either manually or via a secret-population
pipeline) and consumed at pod startup via `envFrom: secretRef`. The External
Secrets Operator itself is installed by Terraform in `infra/terraform/envs/prod`
via Helm and runs once per shared cluster.

## What's not here

- MediaMTX StatefulSet + per-pod LB services — see `SPEC-011b`
- Grafana / Loki / Tempo / Alloy manifests — see `SPEC-011c`
- Runbooks (staging smoke, prod rollout, incident triage) — see `SPEC-011d`
