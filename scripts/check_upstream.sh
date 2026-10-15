#!/usr/bin/env bash
# Report whether upstream Diablo4Companion moved past the last reviewed commit.
#
# remote_head is always the live head of the configured upstream branch
# (`git ls-remote` against url/branch from upstream.lock). The checked-out
# submodule commit is reported separately as local_submodule_head; it is
# never mistaken for the remote head.
#
# Exit codes: 0 = unchanged, 1 = upstream changed (a parity review is due),
# 2 = state unknown (upstream.lock unreadable, git or network failure,
# missing branch, timeout). An unknown state is never reported as checked.
#
# Environment: D4LC_UPSTREAM_TIMEOUT (seconds for ls-remote, default 60).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

lock_value() {
  [ -f upstream.lock ] || return 0
  grep -E "^$1[[:space:]]*=" upstream.lock | head -n 1 \
    | sed -E 's/^[^=]+= *"?([^" ]+)"?.*$/\1/' || true
}

unknown_reasons=()
upstream_url="$(lock_value url)"
upstream_branch="$(lock_value branch)"
last_reviewed_commit="$(lock_value last_reviewed_commit)"
[ -n "$upstream_url" ] || unknown_reasons+=("upstream.lock names no url")
[ -n "$upstream_branch" ] || unknown_reasons+=("upstream.lock names no branch")
[[ "$last_reviewed_commit" =~ ^[0-9a-f]{40}$ ]] \
  || unknown_reasons+=("upstream.lock has no valid last_reviewed_commit")

remote_head="UNKNOWN"
if [ -n "$upstream_url" ] && [ -n "$upstream_branch" ]; then
  ls_remote_err="$(mktemp)"
  trap 'rm -f "$ls_remote_err"' EXIT
  status=0
  ls_remote_out="$(GIT_TERMINAL_PROMPT=0 timeout "${D4LC_UPSTREAM_TIMEOUT:-60}" \
    git ls-remote --exit-code "$upstream_url" "refs/heads/$upstream_branch" 2>"$ls_remote_err")" \
    || status=$?
  if [ "$status" -eq 0 ]; then
    if [[ "$ls_remote_out" =~ ^([0-9a-f]{40})[[:space:]]+refs/heads/[^[:space:]]+$ ]]; then
      remote_head="${BASH_REMATCH[1]}"
    else
      unknown_reasons+=("unexpected ls-remote answer: ${ls_remote_out//$'\n'/; }")
    fi
  elif [ "$status" -eq 2 ]; then
    unknown_reasons+=("branch $upstream_branch does not exist at $upstream_url")
  elif [ "$status" -eq 124 ]; then
    unknown_reasons+=("ls-remote of $upstream_url timed out after ${D4LC_UPSTREAM_TIMEOUT:-60}s")
  else
    unknown_reasons+=("ls-remote of $upstream_url failed (exit $status): $(tr '\n' ' ' <"$ls_remote_err")")
  fi
fi

# An un-initialized submodule leaves an empty directory whose git calls
# would resolve to THIS repository's HEAD, so require its own .git.
local_submodule_head="NOT_INITIALIZED"
if [ -e upstream/Diablo4Companion/.git ]; then
  local_submodule_head="$(git -C upstream/Diablo4Companion rev-parse HEAD 2>/dev/null)" \
    || local_submodule_head="UNREADABLE"
fi

changed="UNKNOWN"
if [ "${#unknown_reasons[@]}" -eq 0 ]; then
  if [ "$remote_head" = "$last_reviewed_commit" ]; then
    changed="false"
  else
    changed="true"
  fi
fi

printf 'UPSTREAM_CHANGE_REPORT\n'
printf 'upstream_url=%s\n' "${upstream_url:-UNKNOWN}"
printf 'upstream_branch=%s\n' "${upstream_branch:-UNKNOWN}"
printf 'remote_head=%s\n' "$remote_head"
printf 'local_submodule_head=%s\n' "$local_submodule_head"
printf 'last_reviewed_commit=%s\n' "${last_reviewed_commit:-UNKNOWN}"
printf 'UPSTREAM_CHANGED=%s\n' "$changed"

case "$changed" in
  false) exit 0 ;;
  true)
    echo "::warning::upstream $upstream_branch moved to $remote_head (last reviewed $last_reviewed_commit); a parity review is due" >&2
    exit 1
    ;;
  *)
    for reason in "${unknown_reasons[@]}"; do
      echo "::error::check_upstream: $reason" >&2
    done
    exit 2
    ;;
esac
