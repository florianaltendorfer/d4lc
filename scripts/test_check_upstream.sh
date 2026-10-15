#!/usr/bin/env bash
# Runs scripts/check_upstream.sh against local git repositories only: a
# throwaway copy of the script with its own upstream.lock and submodule
# directory, and a bare repository as the "remote". No network access.
set -euo pipefail
cd "$(dirname "$0")/.."
script="$PWD/scripts/check_upstream.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

fail() {
    echo "test_check_upstream: $*" >&2
    exit 1
}

git_q() { git -c user.name=t -c user.email=t@example.invalid -c init.defaultBranch=master "$@" >/dev/null 2>&1; }

# The "remote": a bare repo with two commits on master.
git_q init "$work/src"
git_q -C "$work/src" commit --allow-empty -m reviewed
reviewed="$(git -C "$work/src" rev-parse HEAD)"
git_q -C "$work/src" commit --allow-empty -m newer
newer="$(git -C "$work/src" rev-parse HEAD)"
git_q clone --bare "$work/src" "$work/remote.git"

# One checkout layout per case: scripts/check_upstream.sh, upstream.lock,
# and optionally an initialized submodule pinned at the reviewed commit.
case_dir=""
new_case() {
    case_dir="$work/case-$1"
    mkdir -p "$case_dir/scripts" "$case_dir/upstream/Diablo4Companion"
    cp "$script" "$case_dir/scripts/"
    cat >"$case_dir/upstream.lock" <<EOF
url = "${2}"
branch = "${3:-master}"
last_reviewed_commit = "$reviewed"
EOF
}
init_submodule() {
    rm -rf "$case_dir/upstream/Diablo4Companion"
    git_q clone "$work/src" "$case_dir/upstream/Diablo4Companion"
    git_q -C "$case_dir/upstream/Diablo4Companion" checkout --detach "$reviewed"
}

status=0
run_check() {
    status=0
    D4LC_UPSTREAM_TIMEOUT="${TIMEOUT:-20}" "$case_dir/scripts/check_upstream.sh" \
        >"$case_dir/out" 2>"$case_dir/err" || status=$?
}
expect() {
    local label=$1 code=$2 changed=$3 remote=$4 local_head=$5
    [ "$status" -eq "$code" ] || fail "$label: exit $status, expected $code: $(cat "$case_dir/out" "$case_dir/err")"
    grep -qx "UPSTREAM_CHANGED=$changed" "$case_dir/out" || fail "$label: not UPSTREAM_CHANGED=$changed: $(cat "$case_dir/out")"
    grep -qx "remote_head=$remote" "$case_dir/out" || fail "$label: remote_head is not $remote: $(cat "$case_dir/out")"
    grep -qx "local_submodule_head=$local_head" "$case_dir/out" \
        || fail "$label: local_submodule_head is not $local_head: $(cat "$case_dir/out")"
}

# (a) Submodule pinned at the reviewed commit, remote moved on: the live
#     remote head is reported and the change is flagged (the old script
#     reported the submodule commit and "false").
new_case pinned-remote-newer "file://$work/remote.git"
init_submodule
run_check
expect "pinned, remote newer" 1 true "$newer" "$reviewed"
grep -q "::warning::" "$case_dir/err" || fail "pinned, remote newer: no warning annotation"

# (a') Same, but the remote is unreachable: unknown, never "unchanged".
new_case pinned-unreachable "file://$work/missing.git"
init_submodule
run_check
expect "pinned, unreachable" 2 UNKNOWN UNKNOWN "$reviewed"
grep -q "::error::check_upstream: ls-remote" "$case_dir/err" || fail "pinned, unreachable: reason not named"

# (b) Submodule not initialized (the CI checkout) and remote at the
#     reviewed commit: unchanged.
git -C "$work/remote.git" update-ref refs/heads/master "$reviewed"
new_case uninitialized-unchanged "file://$work/remote.git"
run_check
expect "uninitialized, unchanged" 0 false "$reviewed" NOT_INITIALIZED
git -C "$work/remote.git" update-ref refs/heads/master "$newer"

# (b') Not initialized, remote newer.
new_case uninitialized-newer "file://$work/remote.git"
run_check
expect "uninitialized, newer" 1 true "$newer" NOT_INITIALIZED

# (c) Invalid URL.
new_case invalid-url "not a url at all"
run_check
expect "invalid url" 2 UNKNOWN UNKNOWN NOT_INITIALIZED

# (d) Missing branch.
new_case missing-branch "file://$work/remote.git" no-such-branch
run_check
expect "missing branch" 2 UNKNOWN UNKNOWN NOT_INITIALIZED
grep -q "does not exist" "$case_dir/err" || fail "missing branch: reason not named"

# (e) A remote that never answers: bounded by the timeout, unknown.
new_case hanging "ssh://hang.invalid/repo.git"
start=$SECONDS
GIT_SSH_COMMAND='sh -c "sleep 30" --' TIMEOUT=2 run_check
[ $((SECONDS - start)) -lt 15 ] || fail "hanging: ls-remote was not bounded by the timeout"
expect "hanging remote" 2 UNKNOWN UNKNOWN NOT_INITIALIZED
grep -q "timed out" "$case_dir/err" || fail "hanging: timeout not named"

# (f) upstream.lock without a reviewed commit.
new_case no-reviewed "file://$work/remote.git"
sed -i '/^last_reviewed_commit/d' "$case_dir/upstream.lock"
run_check
expect "no reviewed commit" 2 UNKNOWN "$newer" NOT_INITIALIZED

echo "test_check_upstream: ok"
