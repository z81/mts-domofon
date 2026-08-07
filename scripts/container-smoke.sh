#!/usr/bin/env bash

set -euo pipefail

: "${DOMRU_MTS_IMAGE:?DOMRU_MTS_IMAGE must name the image to test}"

VDOME_HOST_PORT="${VDOME_HOST_PORT:-3000}"
export DOMRU_MTS_IMAGE VDOME_HOST_PORT

DOMRU_MTS_SMOKE_PROJECT="${DOMRU_MTS_SMOKE_PROJECT:-domru-mts-smoke-$$-${RANDOM}}"
smoke_project=$DOMRU_MTS_SMOKE_PROJECT
if [[ ! "$smoke_project" =~ ^domru-mts-smoke-[a-z0-9][a-z0-9_-]*$ ]]; then
  printf 'container smoke: invalid DOMRU_MTS_SMOKE_PROJECT\n' >&2
  exit 1
fi

fail() {
  printf 'container smoke: %s\n' "$*" >&2
  exit 1
}

cleanup() {
  status=$?
  trap - EXIT
  compose down --volumes --remove-orphans >/dev/null 2>&1 || true
  exit "$status"
}

assert_equal() {
  actual=$1
  expected=$2
  description=$3
  [ "$actual" = "$expected" ] ||
    fail "$description: expected '$expected', got '$actual'"
}

inspect_value() {
  docker inspect --format "$1" "$container_id"
}

compose() {
  docker compose --project-name "$smoke_project" "$@"
}

assert_absent() {
  description=$1
  shift
  if grep "$@"; then
    fail "$description"
  else
    grep_status=$?
    if [ "$grep_status" -ne 1 ]; then
      printf 'container smoke: grep failed with status %s while checking %s\n' \
        "$grep_status" "$description" >&2
      return "$grep_status"
    fi
  fi
}

command -v docker >/dev/null 2>&1 || fail 'docker is not installed'
command -v curl >/dev/null 2>&1 || fail 'curl is not installed'

compose config --quiet
trap cleanup EXIT

compose up --no-build --wait --wait-timeout 90 --detach domru-mts
container_id="$(compose ps --quiet domru-mts)"
[ -n "$container_id" ] || fail 'Compose did not return a domru-mts container ID'

health_body="$(curl --noproxy '*' --fail --silent --show-error --max-time 5 \
  "http://127.0.0.1:${VDOME_HOST_PORT}/health/live")"
assert_equal "$health_body" '{"status":"ok"}' 'health/live response'
curl --noproxy '*' --fail --silent --show-error --max-time 5 \
  "http://127.0.0.1:${VDOME_HOST_PORT}/" |
  grep -Fqi '<!doctype html' || fail 'web root does not contain <!doctype html'

assert_equal "$(inspect_value '{{.Config.User}}')" '10001:10001' 'container user'
assert_equal "$(inspect_value '{{.HostConfig.ReadonlyRootfs}}')" 'true' 'read-only root filesystem'
assert_equal "$(inspect_value '{{.HostConfig.Privileged}}')" 'false' 'privileged mode'

cap_drop="$(inspect_value '{{range .HostConfig.CapDrop}}{{println .}}{{end}}')"
printf '%s\n' "$cap_drop" | grep -Fxq 'ALL' || fail 'ALL is not present in CapDrop'
security_opt="$(inspect_value '{{range .HostConfig.SecurityOpt}}{{println .}}{{end}}')"
printf '%s\n' "$security_opt" | grep -Fq 'no-new-privileges' ||
  fail 'no-new-privileges is not present in SecurityOpt'

host_ip="$(inspect_value '{{(index (index .NetworkSettings.Ports "3000/tcp") 0).HostIp}}')"
assert_equal "$host_ip" '127.0.0.1' 'published port host IP'

tmpfs_options="$(inspect_value '{{index .HostConfig.Tmpfs "/tmp"}}')"
for option in rw nodev nosuid noexec size=536870912; do
  case ",$tmpfs_options," in
    *",$option,"*) ;;
    *) fail "/tmp tmpfs is missing option '$option': $tmpfs_options" ;;
  esac
done

data_mount_type="$(inspect_value '{{range .Mounts}}{{if eq .Destination "/var/lib/domru-mts"}}{{.Type}}{{end}}{{end}}')"
assert_equal "$data_mount_type" 'volume' '/var/lib/domru-mts mount type'

compose exec -T --user 0:0 domru-mts sh -eu -c '
  if touch /root/.domru-mts-root-write-probe 2>/dev/null; then
    rm -f /root/.domru-mts-root-write-probe
    echo "root filesystem accepted a write" >&2
    exit 1
  fi
'

# The container shell, not this host shell, must expand the probe paths and IDs.
# shellcheck disable=SC2016
compose exec -T domru-mts sh -eu -c '
  test ! -e /var/run/docker.sock
  tmp_probe="/tmp/.domru-mts-write-probe.$$"
  data_probe="/var/lib/domru-mts/.domru-mts-write-probe.$$"
  touch "$tmp_probe" "$data_probe"
  rm "$tmp_probe" "$data_probe"
  test "$(stat -c "%u:%g %a" /var/lib/domru-mts/master.key)" = "10001:10001 600"
  test "$(stat -c "%u:%g %a" /var/lib/domru-mts/device.enc)" = "10001:10001 600"
  ffmpeg -hide_banner -encoders 2>/dev/null | grep -Eq "[[:space:]]libx264[[:space:]]"
  ffmpeg -hide_banner -encoders 2>/dev/null | grep -Eq "[[:space:]]aac[[:space:]]"
  ffmpeg -hide_banner -muxers 2>/dev/null | grep -Eq "[[:space:]]hls[[:space:]]"
  ffmpeg -hide_banner -demuxers 2>/dev/null | grep -Eq "[[:space:]]concat[[:space:]]"
  ! command -v cargo >/dev/null 2>&1
  ! command -v node >/dev/null 2>&1
'

master_hash_before="$(compose exec -T domru-mts \
  sha256sum /var/lib/domru-mts/master.key | awk '{print $1}')"
device_hash_before="$(compose exec -T domru-mts \
  sha256sum /var/lib/domru-mts/device.enc | awk '{print $1}')"

compose rm --stop --force domru-mts
compose up --no-build --wait --wait-timeout 90 --detach domru-mts
container_id="$(compose ps --quiet domru-mts)"
[ -n "$container_id" ] || fail 'Compose did not recreate the domru-mts container'

master_hash_after="$(compose exec -T domru-mts \
  sha256sum /var/lib/domru-mts/master.key | awk '{print $1}')"
device_hash_after="$(compose exec -T domru-mts \
  sha256sum /var/lib/domru-mts/device.enc | awk '{print $1}')"
assert_equal "$master_hash_after" "$master_hash_before" 'master.key persistence hash'
assert_equal "$device_hash_after" "$device_hash_before" 'device.enc persistence hash'

compose stop --timeout 45 domru-mts
assert_equal "$(inspect_value '{{.State.ExitCode}}')" '0' 'container exit code'
assert_equal "$(inspect_value '{{.State.OOMKilled}}')" 'false' 'container OOMKilled state'

state_error="$(inspect_value '{{.State.Error}}')"
service_logs="$(compose logs --no-color domru-mts)"
termination_evidence="${state_error}
${service_logs}"
assert_absent 'container state or logs contain a forced-kill indication' \
  -Eqi '(^|[^[:alpha:]])(SIGKILL|forced[- ]?kill|killed)([^[:alpha:]]|$)' \
  <<<"$termination_evidence"

printf 'container smoke: all checks passed for %s\n' "$DOMRU_MTS_IMAGE"
