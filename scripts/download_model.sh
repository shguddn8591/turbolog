#!/usr/bin/env bash
# Downloads all-MiniLM-L6-v2 ONNX model + tokenizer → models/
set -euo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)/models"
mkdir -p "$DIR"
BASE="https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main"

[ -f "$DIR/model.onnx" ] || curl -L --fail -o "$DIR/model.onnx" "$BASE/onnx/model.onnx"
[ -f "$DIR/tokenizer.json" ] || curl -L --fail -o "$DIR/tokenizer.json" "$BASE/tokenizer.json"

echo "OK: $(ls -lh "$DIR" | tail -n +2)"
