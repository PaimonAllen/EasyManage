#!/usr/bin/env bash
set -Eeuo pipefail

if [[ ! -r /etc/os-release ]]; then
  printf 'Cannot identify this operating system.\n' >&2
  exit 1
fi

# shellcheck disable=SC1091
source /etc/os-release
if [[ "${ID:-}" != "ubuntu" ]]; then
  printf 'This script supports Ubuntu only; detected: %s\n' "${ID:-unknown}" >&2
  exit 1
fi

if (( EUID != 0 )); then
  if ! command -v sudo >/dev/null 2>&1; then
    printf 'Administrator privileges are required and sudo is unavailable.\n' >&2
    exit 1
  fi
  exec sudo bash "$0" "$@"
fi

apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install --yes \
  build-essential \
  ca-certificates \
  clang \
  clang-format \
  cmake \
  curl \
  git \
  jq \
  libsqlite3-dev \
  libssl-dev \
  ninja-build \
  pkg-config \
  postgresql-client \
  protobuf-compiler \
  shellcheck \
  sqlite3 \
  xz-utils

