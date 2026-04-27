#!/usr/bin/env bash
# shellcheck shell=bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
OUT_DIR="${1:-$ROOT_DIR/crates/stygian-charon/docs/examples/fixtures}"
GEN_VERSION="${2:-v1}"

SOURCES=(
  "$ROOT_DIR/crates/stygian-charon/docs/examples/fingerprint-snapshot-v1-http.json"
  "$ROOT_DIR/crates/stygian-charon/docs/examples/fingerprint-snapshot-v1-browser.json"
)

mkdir -p "$OUT_DIR"

normalize_one() {
  local src="$1"
  local dst="$2"
  local source_rel
  source_rel="${src#"$ROOT_DIR"/}"

  python3 - "$src" "$dst" "$source_rel" "$GEN_VERSION" <<'PY'
import json
import sys
from pathlib import Path

src = Path(sys.argv[1])
dst = Path(sys.argv[2])
source_rel = sys.argv[3]
generation_version = sys.argv[4]

with src.open("r", encoding="utf-8") as f:
    snap = json.load(f)

metadata = snap.get("metadata")
if not isinstance(metadata, dict):
    metadata = {}

# Deterministic fixture metadata required by CHR-004.
metadata["fixture_source"] = source_rel
metadata["fixture_generation_version"] = generation_version

# Strip volatile metadata keys so generated fixtures remain byte-stable.
for key in ["capture_nonce", "generated_at", "request_id", "run_id", "session_id", "trace_id"]:
    metadata.pop(key, None)

snap["metadata"] = metadata

payload = json.dumps(snap, indent=2, sort_keys=True) + "\n"
dst.write_text(payload, encoding="utf-8")
PY
}

for src in "${SOURCES[@]}"; do
  if [[ ! -f "$src" ]]; then
    echo "missing source fixture input: $src" >&2
    exit 1
  fi

  base_name="$(basename "$src" .json)"
  normalize_one "$src" "$OUT_DIR/${base_name}.fixture.json"
done

python3 - "$OUT_DIR" "$ROOT_DIR" "$GEN_VERSION" "${SOURCES[@]}" <<'PY'
import json
import sys
from pathlib import Path

out_dir = Path(sys.argv[1])
root_dir = Path(sys.argv[2])
generation_version = sys.argv[3]
sources = [Path(s).relative_to(root_dir).as_posix() for s in sys.argv[4:]]

manifest = {
    "generation_version": generation_version,
    "sources": sorted(sources),
    "fixtures": sorted([p.name for p in out_dir.glob("*.fixture.json")]),
}

(out_dir / "manifest.json").write_text(
    json.dumps(manifest, indent=2, sort_keys=True) + "\n",
    encoding="utf-8",
)
PY

echo "Generated Charon fixtures in: $OUT_DIR"
