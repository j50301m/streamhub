#!/usr/bin/env bash
# streamhub — manual deploy entrypoint for GKE.
#
# Usage:
#   deploy/gke/scripts/deploy.sh <env> <image-tag>
#
#   env        prod | staging
#   image-tag  container image tag (typically the full git SHA)
#
# The script:
#   1. validates `gcloud` context and that kubectl targets the expected cluster
#   2. points each Kustomize image reference at Artifact Registry@<image-tag>
#   3. applies namespace manifests first, then the environment overlay
#   4. waits for rollouts to finish (prints status on failure)
#
# It never deploys more than one env at a time. Rollback uses deploy/gke/scripts/rollback.sh.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." &>/dev/null && pwd)"
GKE_DIR="$REPO_ROOT/deploy/gke"

log() { printf '[deploy] %s\n' "$*" >&2; }
die() { printf '[deploy] ERROR: %s\n' "$*" >&2; exit 1; }

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

require_cmd kubectl
require_cmd kustomize
require_cmd gcloud

ENV_NAME="${1:-}"
IMAGE_TAG="${2:-${IMAGE_TAG:-}}"

case "$ENV_NAME" in
  prod|staging) ;;
  "") die "usage: $0 <prod|staging> <image-tag>" ;;
  *) die "unknown env: $ENV_NAME (expected prod or staging)" ;;
esac

[ -n "$IMAGE_TAG" ] || die "image tag required (argv2 or IMAGE_TAG env)"

OVERLAY_DIR="$GKE_DIR/$ENV_NAME"
[ -d "$OVERLAY_DIR" ] || die "overlay directory missing: $OVERLAY_DIR"

PROJECT_ID="${GCP_PROJECT_ID:-$(gcloud config get-value project 2>/dev/null || true)}"
[ -n "$PROJECT_ID" ] || die "GCP_PROJECT_ID not set and gcloud has no project configured"

CLUSTER_NAME="${GKE_CLUSTER_NAME:-streamhub-prod}"
CLUSTER_REGION="${GKE_CLUSTER_REGION:-asia-east1}"

# Current kube-context must match the expected cluster so we never apply to the wrong place.
EXPECTED_CTX="gke_${PROJECT_ID}_${CLUSTER_REGION}_${CLUSTER_NAME}"
CURRENT_CTX="$(kubectl config current-context)"
if [ "$CURRENT_CTX" != "$EXPECTED_CTX" ]; then
  log "current kube-context is '$CURRENT_CTX', expected '$EXPECTED_CTX'"
  log "run: gcloud container clusters get-credentials $CLUSTER_NAME --region $CLUSTER_REGION --project $PROJECT_ID"
  die "refusing to deploy to wrong cluster"
fi

NAMESPACE="streamhub-$ENV_NAME"

log "env=$ENV_NAME project=$PROJECT_ID cluster=$CLUSTER_NAME region=$CLUSTER_REGION tag=$IMAGE_TAG ns=$NAMESPACE"

# Pin image tags in-place; `kustomize edit set image` rewrites the overlay kustomization.yaml.
# We run this inside a temporary workdir so the repo stays clean.
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

cp -r "$GKE_DIR" "$WORK_DIR/gke"
WORK_OVERLAY="$WORK_DIR/gke/$ENV_NAME"

IMAGE_PREFIX="$CLUSTER_REGION-docker.pkg.dev/$PROJECT_ID/streamhub"

pushd "$WORK_OVERLAY" >/dev/null
  for component in api bo-api web; do
    kustomize edit set image "$component=$IMAGE_PREFIX/$component:$IMAGE_TAG"
  done
popd >/dev/null

# Replace PROJECT_ID placeholders in the copied overlay (ServiceAccount annotations,
# ConfigMaps, ClusterSecretStore). Everything else is env-agnostic in base.
find "$WORK_OVERLAY" -type f \( -name '*.yaml' -o -name '*.yml' \) \
  -exec sed -i.bak "s|PROJECT_ID|$PROJECT_ID|g" {} +
find "$WORK_OVERLAY" -name '*.bak' -delete

# Apply namespaces first (they are not namespaced and every other manifest depends on them).
log "applying namespaces"
kustomize build "$WORK_DIR/gke/base/namespaces" | kubectl apply -f -

log "applying overlay $ENV_NAME"
kustomize build "$WORK_OVERLAY" | kubectl apply -f -

log "waiting for rollouts (timeout 300s each)"
for dep in api bo-api web; do
  if ! kubectl -n "$NAMESPACE" rollout status deployment/"$dep" --timeout=300s; then
    log "rollout failed for $dep — capturing pod state"
    kubectl -n "$NAMESPACE" get pods -l "app.kubernetes.io/name=$dep" -o wide
    kubectl -n "$NAMESPACE" describe deployment "$dep" | tail -n 30
    die "deployment $dep failed to become ready"
  fi
done

log "done. verify gateway/addresses with:"
log "  kubectl -n $NAMESPACE get gateway streamhub -o wide"
log "  gcloud certificate-manager certificates list --project $PROJECT_ID"
log "  gcloud certificate-manager maps list --project $PROJECT_ID"
