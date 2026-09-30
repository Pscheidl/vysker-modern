#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ -f .env ]]; then
  set -a
  source .env
  set +a
fi
if command -v cargo-leptos >/dev/null 2>&1; then
  exec cargo-leptos watch "$@"
elif [ -x target/tools/cargo-leptos-x86_64-unknown-linux-gnu/cargo-leptos ]; then
  exec target/tools/cargo-leptos-x86_64-unknown-linux-gnu/cargo-leptos watch "$@"
else
  printf '%s\n' 'Chybí cargo-leptos. Nainstalujte: cargo install cargo-leptos --locked' >&2
  exit 1
fi
