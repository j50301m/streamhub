# Workload Identity bindings

Each application KSA has a matching GSA in `infra/terraform/envs/prod/iam.tf`
(to be added alongside the first `terraform apply`). The KSA annotation
`iam.gke.io/gcp-service-account` points at the GSA; the GSA has an
`iam.gke.io/gcp-service-account` IAM binding (`roles/iam.workloadIdentityUser`)
scoped to `serviceAccount:<project>.svc.id.goog[<ns>/<ksa>]`.

| Kubernetes SA          | Namespace                         | GCP SA (suffix `@<project>.iam.gserviceaccount.com`) | IAM roles                                                                                       |
|------------------------|-----------------------------------|------------------------------------------------------|-------------------------------------------------------------------------------------------------|
| `api`                  | `streamhub-prod` / `-staging`     | `streamhub-api`                                      | `roles/storage.objectAdmin` (recordings/vod/thumbnails buckets), `roles/transcoder.admin`, `roles/pubsub.publisher`, `roles/secretmanager.secretAccessor` (selected), `roles/cloudtrace.agent` |
| `bo-api`               | `streamhub-prod` / `-staging`     | `streamhub-bo-api`                                   | `roles/storage.objectViewer` (recordings/vod), `roles/secretmanager.secretAccessor` (selected), `roles/cloudtrace.agent`                                                                       |
| `web`                  | `streamhub-prod` / `-staging`     | — (no GSA; no GCP API access)                        | —                                                                                               |
| `external-secrets`     | `external-secrets`                | `streamhub-eso`                                      | `roles/secretmanager.secretAccessor` on the whole streamhub secret prefix                                                                                                                      |

## Rotation

Secret Manager secrets are versioned. Rotate by adding a new version; ESO
refreshes every `refreshInterval` (1h by default). For immediate rollout, bump
an annotation on the target Deployment to trigger a new pod spec hash.

## Never

- No service-account JSON keys in any manifest or image.
- No direct `GOOGLE_APPLICATION_CREDENTIALS` file paths in env.
