#!/usr/bin/env bash
# Preserve private host-file permissions by using the host user's UID/GID.
set -euo pipefail

face_probe_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
export FACE_PROBE_UID="${FACE_PROBE_UID:-$(id -u)}"
export FACE_PROBE_GID="${FACE_PROBE_GID:-$(id -g)}"
export FACE_PROBE_MODELS_DIR="${FACE_PROBE_MODELS_DIR:-$face_probe_dir/models}"
export FACE_PROBE_DATA_DIR="${FACE_PROBE_DATA_DIR:-$face_probe_dir/reports/dashboard}"

if [[ "$FACE_PROBE_UID" == "0" ]]; then
  echo 'Run as a non-root user, or set FACE_PROBE_UID/GID to the owner of the mounted directories.' >&2
  exit 1
fi

umask 077
mkdir -p -- "$FACE_PROBE_DATA_DIR"
if [[ -z "${FACE_PROBE_REFERENCES_DIR:-}" ]]; then
  export FACE_PROBE_REFERENCES_DIR="$face_probe_dir/references"
  mkdir -p -- "$FACE_PROBE_REFERENCES_DIR"
fi
if [[ ! -d "$FACE_PROBE_MODELS_DIR" || ! -d "$FACE_PROBE_REFERENCES_DIR" ]]; then
  echo 'Model and reference mounts must be existing directories. See README.md.' >&2
  exit 1
fi
if [[ $# == 0 ]]; then
  echo 'Usage: bash scripts/face-probe/service.sh up --build -d  (or logs, stop, down, config)' >&2
  exit 1
fi
exec docker compose --project-directory "$face_probe_dir" -f "$face_probe_dir/compose.yml" "$@"
