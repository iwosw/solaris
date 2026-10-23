#!/usr/bin/env bash
# Usage: bash tools/extract-block-map-colors.sh /path/to/licensed/server.jar
# Output is ignored registry data, like extract-block-light.sh.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
unzip -qo "${1:?licensed server bundle required}" -d "$TMP/bundle"
mapfile -t JARS < <(find "$TMP/bundle/META-INF" -name '*.jar' -type f)
CP="$(IFS=:; echo "${JARS[*]}")"
JAVA="${JAVA:-java}"
JAVAC="${JAVAC:-javac}"
mkdir -p "$TMP/classes" "$ROOT/data/vanilla/reports"
"$JAVAC" -cp "$CP" -d "$TMP/classes" "$ROOT/tools/extract-block-map-colors/MapColorExtractor.java"
"$JAVA" -cp "$TMP/classes:$CP" MapColorExtractor "$ROOT/data/vanilla/reports/block_map_colors.json"
