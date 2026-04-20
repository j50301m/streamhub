#!/usr/bin/env bash
# Render and diff the rendered overlay against the live cluster.
#
# Usage:
#   deploy/gke/scripts/diff.sh <env>
#
# Uses `kubectl diff` which requires read + diff permissions. Image tags stay
# at their placeholder values; use deploy.sh for a real apply.

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/../../.." &>/dev/null && pwd)"

ENV_NAME="${1:-}"
case "$ENV_NAME" in
  prod|staging) ;;
  *) echo "usage: $0 <prod|staging>" >&2; exit 1 ;;
esac

kustomize build "$REPO_ROOT/deploy/gke/$ENV_NAME" | kubectl diff -f - || true
