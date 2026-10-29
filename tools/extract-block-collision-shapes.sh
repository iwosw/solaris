#!/usr/bin/env bash
# Extract state-specific collision boxes (or normalized outline boxes with
# --outline) from vanilla 26.1.2. Both tables use the same embedded mc-data format.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECK=0
OUTLINE=0
EXTRACTOR_ARGS=()
while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --check) CHECK=1 ;;
    --outline) OUTLINE=1; EXTRACTOR_ARGS=(--outline) ;;
    *) echo "error: unknown option $1" >&2; exit 2 ;;
  esac
  shift
done

BUNDLE_JAR="${1:-$REPO_ROOT/.analysis/server.jar}"
OUT_BIN="$REPO_ROOT/crates/mc-data/data/block_collision_shapes_26_3.bin"
if [[ "$OUTLINE" -eq 1 ]]; then
  OUT_BIN="$REPO_ROOT/crates/mc-data/data/block_outline_shapes_26_3.bin"
fi
JAVA="${JAVA:-$REPO_ROOT/.analysis/java/current/bin/java}"
if [[ ! -x "$JAVA" ]]; then
  JAVA="$(command -v java || true)"
fi
JAVAC="${JAVA%/java}/javac"

if [[ ! -f "$BUNDLE_JAR" ]]; then
  echo "error: bundle jar not found at $BUNDLE_JAR" >&2
  exit 1
fi
if [[ ! -x "$JAVA" ]] || [[ ! -x "$JAVAC" ]]; then
  echo "error: java/javac not found (java=$JAVA)" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

unzip -qo "$BUNDLE_JAR" -d "$TMP/bundle"
INNER_JAR="$(find "$TMP/bundle/META-INF/versions" -name 'server-*.jar' | head -n1)"
if [[ -z "$INNER_JAR" ]]; then
  echo "error: could not locate inner server-<version>.jar" >&2
  exit 1
fi

LIB_CP="$(find "$TMP/bundle/META-INF/libraries" -name '*.jar' | paste -sd:)"
CP="$INNER_JAR:$LIB_CP"
mkdir -p "$TMP/classes"
"$JAVAC" -d "$TMP/classes" -cp "$CP" \
  "$REPO_ROOT/tools/extract-block-collision-shapes/CollisionShapeExtractor.java"

GENERATED="$TMP/block_collision_shapes_26_3.bin"
"$JAVA" -cp "$TMP/classes:$CP" CollisionShapeExtractor "${EXTRACTOR_ARGS[@]}" "$GENERATED"

if [[ "$CHECK" -eq 1 ]]; then
  if ! cmp -s "$GENERATED" "$OUT_BIN"; then
    echo "error: $OUT_BIN is stale; rerun tools/extract-block-collision-shapes.sh" >&2
    exit 1
  fi
  echo "verified $OUT_BIN against bundled vanilla 26.1.2"
else
  mkdir -p "$(dirname "$OUT_BIN")"
  cp "$GENERATED" "$OUT_BIN"
  echo "wrote $OUT_BIN ($(stat -c %s "$OUT_BIN") bytes)"
fi
