#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

demo_base="${1:-/}"
if [[ ! "$demo_base" =~ ^/([A-Za-z0-9._~-]+/)*$ ]]; then
  printf '%s\n' 'Cesta musí začínat i končit lomítkem, například /obecni-web/.' >&2
  exit 1
fi

if command -v trunk >/dev/null 2>&1; then
  demo_trunk="$(command -v trunk)"
elif [ -x target/tools/trunk/trunk ]; then
  demo_trunk="target/tools/trunk/trunk"
else
  printf '%s\n' 'Chybí Trunk. Nainstalujte: cargo install trunk --locked --version 0.21.14' >&2
  exit 1
fi

env -u NO_COLOR "$demo_trunk" build --release --cargo-profile wasm-release --public-url "$demo_base"
