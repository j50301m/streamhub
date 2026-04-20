#!/usr/bin/env bash
# streamhub — manual rollback entrypoint for GKE.
#
# Usage:
#   deploy/gke/scripts/rollback.sh <env> [deployment]
#
#   env         prod | staging
#   deployment  optional: api | bo-api | web (default: all three)
#
# Rolls back to the previous ReplicaSet using `kubectl rollout undo`.
# For image-level rollback to a specific tag, re-run deploy.sh with the desired tag.

set -euo pipefail

log() { printf '[rollback] %s\n' "$*" >&2; }
die() { printf '[rollback] ERROR: %s\n' "$*" >&2; exit 1; }

command -v kubectl >/dev/null 2>&1 || die "kubectl not found"

ENV_NAME="${1:-}"
TARGET="${2:-all}"

case "$ENV_NAME" in
  prod|staging) ;;
  "") die "usage: $0 <prod|staging> [deployment]" ;;
  *) die "unknown env: $ENV_NAME" ;;
esac

NAMESPACE="streamhub-$ENV_NAME"

case "$TARGET" in
  all)      TARGETS=(api bo-api web) ;;
  api|bo-api|web) TARGETS=("$TARGET") ;;
  *) die "unknown deployment: $TARGET (expected api, bo-api, web, or omit for all)" ;;
esac

for dep in "${TARGETS[@]}"; do
  log "rolling back deployment/$dep in ns/$NAMESPACE"
  kubectl -n "$NAMESPACE" rollout undo deployment/"$dep"
done

for dep in "${TARGETS[@]}"; do
  kubectl -n "$NAMESPACE" rollout status deployment/"$dep" --timeout=300s || {
    log "rollback did not complete for $dep — inspect with: kubectl -n $NAMESPACE get pods -l app.kubernetes.io/name=$dep"
    exit 1
  }
done

log "rollback complete. Current images:"
for dep in "${TARGETS[@]}"; do
  kubectl -n "$NAMESPACE" get deployment "$dep" -o jsonpath='{.metadata.name}{"\t"}{.spec.template.spec.containers[0].image}{"\n"}'
done
