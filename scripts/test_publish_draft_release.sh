#!/usr/bin/env bash
# Runs scripts/publish_draft_release.sh against a fake `gh` on PATH. No
# real tag or release is read or changed. The fake answers from per-case
# files (applying the real `jq` to `--jq` filters, as gh does) and logs
# every invocation, so each case can assert which release mutations
# happened.
set -euo pipefail
cd "$(dirname "$0")/.."
repo_root=$PWD

work=$(mktemp -d)
trap '[ -n "${KEEP_WORK:-}" ] || rm -rf "$work"' EXIT

fail() {
    echo "test_publish_draft_release: $*" >&2
    exit 1
}

command -v jq >/dev/null || fail "jq is required"

version="$(awk -F'"' '/^version = / { print $2; exit }' crates/d4lc-app/Cargo.toml)"
tag="v$version"
build_sha=1111111111111111111111111111111111111111
other_sha=2222222222222222222222222222222222222222
tag_object=3333333333333333333333333333333333333333

mkdir -p "$work/bin" "$work/artifacts"
cat >"$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
# Fake gh: answers from $FAKE_GH_DIR, logs every call.
printf '%s\n' "$*" >>"$FAKE_GH_DIR/calls.log"
# Answer file N of a sequence: <name>.<N> if present, else the last one.
next_answer() {
    local name=$1 counter="$FAKE_GH_DIR/.$1.count" n
    n=$(( $(cat "$counter" 2>/dev/null || echo 0) + 1 ))
    echo "$n" >"$counter"
    while [ "$n" -gt 1 ] && ! compgen -G "$FAKE_GH_DIR/$name.$n.*" >/dev/null; do
        n=$((n - 1))
    done
    echo "$FAKE_GH_DIR/$name.$n"
}
# Serve one answer: <file>.json on stdout, or <file>.err on stderr with the
# exit code in <file>.code (and optional partial stdout in <file>.out).
serve() {
    local base=$1 filter=$2
    if [ -f "$base.err" ]; then
        [ -f "$base.out" ] && cat "$base.out"
        cat "$base.err" >&2
        exit "$(cat "$base.code" 2>/dev/null || echo 1)"
    fi
    [ -f "$base.json" ] || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
    if [ -n "$filter" ]; then jq -r "$filter" "$base.json"; else cat "$base.json"; fi
}
if [ "$1" = api ]; then
    shift
    endpoint="" filter=""
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --jq) filter=$2; shift 2 ;;
            --paginate) shift ;;
            -*) echo "fake gh: unexpected api flag $1" >&2; exit 1 ;;
            *) endpoint=$1; shift ;;
        esac
    done
    case "$endpoint" in
        repos/*/git/ref/tags/*) serve "$FAKE_GH_DIR/ref" "$filter" ;;
        repos/*/git/tags/*) serve "$FAKE_GH_DIR/tag-object" "$filter" ;;
        "repos/"*"/releases?per_page=100") serve "$(next_answer list)" "$filter" ;;
        repos/*/releases/[0-9]*) serve "$(next_answer release)" "$filter" ;;
        *) echo "fake gh: unexpected api endpoint $endpoint" >&2; exit 1 ;;
    esac
    exit 0
fi
case "$1 $2" in
    "release upload" | "release edit" | "release create" | "release delete")
        exit 0 ;;
    *)
        echo "fake gh: unexpected call: $*" >&2
        exit 1 ;;
esac
EOF
chmod +x "$work/bin/gh"

printf 'tarball' >"$work/artifacts/d4lc.tar.gz"
printf 'deb' >"$work/artifacts/d4lc.deb"
(cd "$work/artifacts" && sha256sum d4lc.tar.gz d4lc.deb >SHA256SUMS)
printf '# notes\n' >"$work/notes.md"

case_dir=""
new_case() {
    case_dir="$work/case-$1"
    mkdir -p "$case_dir"
    : >"$case_dir/calls.log"
    cat >"$case_dir/summary.txt" <<EOF
PACKAGE_LINUX_RELEASE
version=$version
git_revision=$build_sha
EOF
}

# Args: optional tag; $PUBLISH_CWD overrides the checkout the script reads.
run_publish() {
    local tag_arg=${1:-$tag}
    (
        cd "${PUBLISH_CWD:-$repo_root}"
        FAKE_GH_DIR="$case_dir" GH_REPO=owner/d4lc PATH="$work/bin:$PATH" \
            "$repo_root/scripts/publish_draft_release.sh" \
            --tag "$tag_arg" --sha "$build_sha" --version "$version" \
            --summary "$case_dir/summary.txt" --notes "$work/notes.md" \
            "$work/artifacts/d4lc.tar.gz" "$work/artifacts/d4lc.deb" \
            "$work/artifacts/SHA256SUMS" "$case_dir/summary.txt"
    ) >"$case_dir/out.log" 2>&1
}

commit_ref() {
    printf '{"object":{"type":"commit","sha":"%s"}}' "$1" >"$case_dir/ref.json"
}

# Release list answer N (default 1): a JSON array of {id, tag_name, draft}.
releases() {
    printf '%s' "$1" >"$case_dir/list.${2:-1}.json"
}

# Per-release re-read answer N: draft flag and tag.
release_state() {
    printf '{"id":7,"draft":%s,"tag_name":"%s"}' "$1" "${3:-$tag}" >"$case_dir/release.${2:-1}.json"
}

list_error() {
    printf '%s\n' "$1" >"$case_dir/list.1.err"
    printf '%s' "${2:-1}" >"$case_dir/list.1.code"
}

assert_no_mutation() {
    if grep -Eq '^release (upload|edit|create|delete)' "$case_dir/calls.log"; then
        fail "$1: a release was mutated: $(tr '\n' ';' <"$case_dir/calls.log")"
    fi
}

draft_entry='{"id":7,"tag_name":"'"$tag"'","draft":true}'
published_entry='{"id":7,"tag_name":"'"$tag"'","draft":false}'
other_entries='{"id":3,"tag_name":"v0.9.0","draft":false},{"id":4,"tag_name":"'"$tag"'-rc1","draft":true}'

# 1. Published release: refused, nothing mutated.
new_case published
commit_ref "$build_sha"
releases "[$other_entries,$published_entry]"
run_publish && fail "published: a published release must be refused"
grep -q "already published" "$case_dir/out.log" || fail "published: refusal not named"
assert_no_mutation published

# 2. Existing draft: assets replaced and notes updated, never re-created or
#    switched to draft; the draft state is re-read around every mutation.
new_case draft
commit_ref "$build_sha"
releases "[$draft_entry]"
release_state true
run_publish || fail "draft: refresh failed: $(cat "$case_dir/out.log")"
grep -q "^release upload $tag .*--clobber" "$case_dir/calls.log" || fail "draft: no upload"
grep -q "^release edit $tag " "$case_dir/calls.log" || fail "draft: notes not updated"
grep -q -- "--draft" <(grep "^release edit" "$case_dir/calls.log") \
    && fail "draft: edit must not toggle the draft flag"
grep -q "^release create" "$case_dir/calls.log" && fail "draft: must not create a release"
[ "$(grep -c '^api repos/owner/d4lc/releases/7 ' "$case_dir/calls.log")" -eq 3 ] \
    || fail "draft: the draft state was not re-read before and after each mutation"

# 3. No release for the tag (only other tags, including a prefix match):
#    a draft is created on the existing tag and then confirmed.
new_case create
commit_ref "$build_sha"
releases "[$other_entries]"
releases "[$other_entries,$draft_entry]" 2
release_state true
run_publish || fail "create: failed: $(cat "$case_dir/out.log")"
grep -q "^release create $tag .*--draft.*--verify-tag" "$case_dir/calls.log" \
    || fail "create: draft not created with --verify-tag: $(cat "$case_dir/calls.log")"
grep -q -- "--paginate" "$case_dir/calls.log" || fail "create: the release list was not paginated"

# 4. Annotated tag is peeled to its commit.
new_case annotated
printf '{"object":{"type":"tag","sha":"%s"}}' "$tag_object" >"$case_dir/ref.json"
printf '{"object":{"type":"commit","sha":"%s"}}' "$build_sha" >"$case_dir/tag-object.json"
releases "[]"
releases "[$draft_entry]" 2
release_state true
run_publish || fail "annotated: failed: $(cat "$case_dir/out.log")"
grep -q "^api repos/owner/d4lc/git/tags/$tag_object" "$case_dir/calls.log" \
    || fail "annotated: tag object not dereferenced"

# 5. Tag on another commit (e.g. main moved on, or an older build): refused
#    before the release state is even read.
new_case moved-tag
commit_ref "$other_sha"
releases "[]"
run_publish && fail "moved-tag: a tag on another commit must be refused"
grep -q "points to $other_sha" "$case_dir/out.log" || fail "moved-tag: mismatch not named"
grep -q "releases" "$case_dir/calls.log" && fail "moved-tag: release state must not be read"
assert_no_mutation moved-tag

# 6. Annotated tag peeling to another commit.
new_case moved-annotated
printf '{"object":{"type":"tag","sha":"%s"}}' "$tag_object" >"$case_dir/ref.json"
printf '{"object":{"type":"commit","sha":"%s"}}' "$other_sha" >"$case_dir/tag-object.json"
releases "[]"
run_publish && fail "moved-annotated: must be refused"
assert_no_mutation moved-annotated

# 7. Missing tag: never created by this script.
new_case missing-tag
releases "[]"
run_publish && fail "missing-tag: a missing tag must be refused"
grep -q "must exist" "$case_dir/out.log" || fail "missing-tag: reason not named"
assert_no_mutation missing-tag

# 8. A release list that cannot be read is never "no release": server
#    error, authentication failure, rate limit, network failure, and a
#    pagination that fails after a first page (the shape in which
#    `gh release view` could report "release not found").
for failure in "gh: Bad Gateway (HTTP 502)|1" "gh: Bad credentials (HTTP 401)|4" \
    "gh: API rate limit exceeded (HTTP 403)|1" \
    "error connecting to api.github.com|1" "release not found|1"; do
    new_case "list-error-${failure##*|}-${#failure}"
    commit_ref "$build_sha"
    list_error "${failure%|*}" "${failure##*|}"
    run_publish && fail "list error '${failure%|*}': an unknown release state must stop the script"
    grep -q "state of $tag is unknown" "$case_dir/out.log" \
        || fail "list error '${failure%|*}': reason not named: $(cat "$case_dir/out.log")"
    assert_no_mutation "list error ${failure%|*}"
done
new_case list-partial-page
commit_ref "$build_sha"
list_error "gh: Server Error (HTTP 500)" 1
printf '3 false\n' >"$case_dir/list.1.out"
run_publish && fail "list-partial-page: a partial release list must stop the script"
assert_no_mutation list-partial-page

# 9. Package summary from another commit or a dirty tree.
for revision in "$other_sha" "${build_sha}-dirty"; do
    new_case "summary-${revision: -5}"
    commit_ref "$build_sha"
    releases "[]"
    sed -i "s/^git_revision=.*/git_revision=$revision/" "$case_dir/summary.txt"
    run_publish && fail "summary $revision: a foreign build must be refused"
    assert_no_mutation "summary $revision"
done

# 10. Tag that does not name the package version.
new_case wrong-tag
commit_ref "$build_sha"
releases "[]"
run_publish "v0.0.1" && fail "wrong-tag: a tag for another version must be refused"
assert_no_mutation wrong-tag

# 11. More than one release for the tag (e.g. two drafts): never guessed.
new_case two-drafts
commit_ref "$build_sha"
releases "[$draft_entry,{\"id\":8,\"tag_name\":\"$tag\",\"draft\":true}]"
run_publish && fail "two-drafts: an ambiguous release state must be refused"
grep -q "2 releases exist for $tag" "$case_dir/out.log" || fail "two-drafts: reason not named"
assert_no_mutation two-drafts

# 12. The draft is published between the list and the upload.
new_case published-before-upload
commit_ref "$build_sha"
releases "[$draft_entry]"
release_state false
run_publish && fail "published-before-upload: must stop"
assert_no_mutation published-before-upload

# 13. The draft is published during the upload: the notes are not edited.
new_case published-during-upload
commit_ref "$build_sha"
releases "[$draft_entry]"
release_state true 1
release_state false 2
run_publish && fail "published-during-upload: must stop"
grep -q "^release edit" "$case_dir/calls.log" && fail "published-during-upload: notes edited"

# 14. After creating the draft, the list shows two releases for the tag
#    (a concurrent creator): reported, not hidden.
new_case create-race
commit_ref "$build_sha"
releases "[]"
releases "[$draft_entry,{\"id\":8,\"tag_name\":\"$tag\",\"draft\":true}]" 2
run_publish && fail "create-race: a duplicate draft must be reported"
grep -q "after creating the draft" "$case_dir/out.log" || fail "create-race: reason not named"

# 15. An artifact that does not match SHA256SUMS, and a missing SHA256SUMS:
#    refused before any GitHub call about the release.
new_case checksum-mismatch
commit_ref "$build_sha"
releases "[]"
cp -r "$work/artifacts" "$case_dir/artifacts"
printf 'tampered' >"$work/artifacts/d4lc.deb"
run_publish && fail "checksum-mismatch: a tampered artifact must be refused"
grep -q "does not match" "$case_dir/out.log" || fail "checksum-mismatch: reason not named"
grep -q "releases" "$case_dir/calls.log" && fail "checksum-mismatch: release state was read"
cp "$case_dir/artifacts/d4lc.deb" "$work/artifacts/d4lc.deb"

new_case no-sums
commit_ref "$build_sha"
releases "[]"
(
    cd "$repo_root"
    FAKE_GH_DIR="$case_dir" GH_REPO=owner/d4lc PATH="$work/bin:$PATH" \
        ./scripts/publish_draft_release.sh \
        --tag "$tag" --sha "$build_sha" --version "$version" \
        --summary "$case_dir/summary.txt" --notes "$work/notes.md" \
        "$work/artifacts/d4lc.tar.gz" "$work/artifacts/d4lc.deb"
) >"$case_dir/out.log" 2>&1 && fail "no-sums: artifacts without SHA256SUMS must be refused"
grep -q "no SHA256SUMS" "$case_dir/out.log" || fail "no-sums: reason not named"
assert_no_mutation no-sums

# 16. The CLI crate (whose version names the packages) and the app crate
#     must both carry the release version.
new_case cli-version
commit_ref "$build_sha"
releases "[]"
fake_checkout="$case_dir/checkout"
mkdir -p "$fake_checkout/crates/d4lc-app" "$fake_checkout/crates/d4lc-cli"
cp crates/d4lc-app/Cargo.toml "$fake_checkout/crates/d4lc-app/"
sed "0,/^version = .*/s//version = \"0.0.1\"/" crates/d4lc-cli/Cargo.toml \
    >"$fake_checkout/crates/d4lc-cli/Cargo.toml"
PUBLISH_CWD="$fake_checkout" run_publish && fail "cli-version: a version split must be refused"
grep -q "d4lc-cli crate version '0.0.1'" "$case_dir/out.log" || fail "cli-version: reason not named"
assert_no_mutation cli-version

# 17. Both creating and refreshing a draft preserve release-channel metadata.
for version in 1.0.0 1.0.0-rc.1 1.0.0+build-test; do
    tag="v$version"
    channel=false
    [[ "${version%%+*}" != *-* ]] || channel=true
    for action in create edit; do
        new_case "channel-$version-$action"
        fake_checkout="$case_dir/checkout"
        for crate in d4lc-app d4lc-cli; do
            mkdir -p "$fake_checkout/crates/$crate"
            printf 'version = "%s"\n' "$version" >"$fake_checkout/crates/$crate/Cargo.toml"
        done
        commit_ref "$build_sha"
        draft_entry='{"id":7,"tag_name":"'"$tag"'","draft":true}'
        releases "[$draft_entry]"
        if [ "$action" = create ]; then
            releases '[]'
            releases "[$draft_entry]" 2
        fi
        release_state true
        PUBLISH_CWD="$fake_checkout" run_publish || fail "channel $version $action: $(cat "$case_dir/out.log")"
        mutation=$(grep "^release $action " "$case_dir/calls.log")
        [[ "$mutation" == *"--prerelease=$channel"* ]] || fail "$version $action: wrong prerelease flag"
        if [ "$channel" = true ]; then
            [[ "$mutation" == *"--latest=false"* ]] || fail "$version $action: RC must not become Latest"
        else
            [[ "$mutation" != *"--latest=false"* ]] || fail "$version $action: stable release blocked from Latest"
        fi
    done
done

echo "test_publish_draft_release: ok"
