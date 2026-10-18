#!/usr/bin/env bash
# Focused real-client bucket/block-resync scenario with a fresh world and seed.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
MODE="${1:---real-client}"

MANIFEST="$REPO_ROOT/docs/real-client-regression/manifests/m94-regression-pack.json"
SCENARIO="m94-02b-rejected-block-resync"
SERVER_SEED="0"

cd "$REPO_ROOT"


run_real_client_check() {
  SOLARIS_REAL_CLIENT_MANIFEST="$MANIFEST" \
  SOLARIS_REAL_CLIENT_AGENT_SCENARIO="$SCENARIO" \
  SOLARIS_REAL_CLIENT_FRESH_WORLD=1 \
  SOLARIS_REAL_CLIENT_SERVER_SEED="$SERVER_SEED" \
    bash "$REPO_ROOT/tools/harness/backends/regression.sh" --check
}

run_real_client() {
  SOLARIS_REAL_CLIENT_MANIFEST="$MANIFEST" \
  SOLARIS_REAL_CLIENT_AGENT_SCENARIO="$SCENARIO" \
  SOLARIS_REAL_CLIENT_FRESH_WORLD=1 \
  SOLARIS_REAL_CLIENT_SERVER_SEED="$SERVER_SEED" \
    bash "$REPO_ROOT/tools/harness/backends/regression.sh" --run
}

case "$MODE" in
  --check)
    run_real_client_check
    ;;
  --real-client)
    run_real_client
    ;;
  -h|--help)
    cat <<'EOF'
Usage: python3 -m tools.harness run bucket-resync [--check|--real-client]

  --check        Validate the exact declared real-client scenario and adapter.
  --real-client  Run the exact graphical real-client scenario. Default.
EOF
    ;;
  *)
    printf 'error: unknown mode: %s\n' "$MODE" >&2
    exit 2
    ;;
esac
