#!/bin/sh
# SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
# SPDX-License-Identifier: Apache-2.0

# Build and deploy commands of Workers Builds (see Deployment in AGENTS.md). The build image has
# Node.js and bun but no Rust, and each command starts with a fresh PATH.
#
#   ./hack/workers-builds.sh build    # rustup if missing, rust-toolchain.toml, worker-build, build/
#   ./hack/workers-builds.sh deploy   # make deploy: D1 migrations, then wrangler deploy (production)

set -eu

cd "$(dirname "$0")/.."

load_cargo_env() {
  if [ -f "$HOME/.cargo/env" ]; then
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
  fi
}

build() {
  load_cargo_env
  if ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
      sh -s -- -y --profile minimal --default-toolchain none
    # shellcheck source=/dev/null
    . "$HOME/.cargo/env"
  fi
  rustup toolchain install
  # Same version as Cargo.toml, wrangler.toml, the Makefile and ci.yml: Renovate moves them together.
  cargo install -q --locked worker-build@0.9.0
  worker-build --release
}

deploy() {
  # wrangler deploy always runs the [build] command of wrangler.toml, which calls cargo.
  load_cargo_env
  bun run migrate:remote
  bun run deploy
}

case "${1:-}" in
build) build ;;
deploy) deploy ;;
*)
  echo "usage: $0 build|deploy" >&2
  exit 2
  ;;
esac
