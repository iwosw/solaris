#!/usr/bin/env bash
# Export bootstrap-only worldgen registries and optional headless seed samples.
# Mojang output remains in ignored data/vanilla and .analysis directories.
# Usage: bash tools/extract-vanilla-worldgen.sh [bundle.jar] [output.json] [seed ...]
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_JAR="${1:-$REPO_ROOT/.analysis/server.jar}"
OUT_JSON="${2:-$REPO_ROOT/.analysis/codex-logs/vanilla-worldgen/oracle.json}"
JAVA="${JAVA:-$(command -v java)}"
JAVAC="${JAVA%/java}/javac"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
unzip -qo "$BUNDLE_JAR" -d "$TMP/bundle"
mapfile -t INNER_JARS < <(find "$TMP/bundle/META-INF/versions" -name 'server-*.jar')
if [[ ${#INNER_JARS[@]} != 1 ]]; then
  echo 'error: expected exactly one bundled server jar' >&2
  exit 1
fi
LIB_CP="$(find "$TMP/bundle/META-INF/libraries" -name '*.jar' | paste -sd:)"
CP="${INNER_JARS[0]}:$LIB_CP"
mkdir -p "$TMP/classes" "$(dirname "$OUT_JSON")"
"$JAVAC" -cp "$CP" -d "$TMP/classes" "$REPO_ROOT/tools/VanillaWorldgenOracle.java"
if (( $# > 2 )); then
  shift 2
else
  set --
fi
"$JAVA" -Xmx1G -cp "$TMP/classes:$CP" VanillaWorldgenOracle "$REPO_ROOT/data/vanilla/data" "$OUT_JSON" "$@"
