#!/usr/bin/env bash
# Create or refresh the draft GitHub Release for one verified build.
#
# Release identity: the tag, the commit the artifacts were built from, the
# package metadata and the crate version must all name the same release.
#   - TAG is v<VERSION> and VERSION is the crate version of both shipped
#     binaries (d4lc-app, d4lc-cli).
#   - The package summary records VERSION and the build commit BUILD_SHA
#     (a dirty tree never matches).
#   - TAG already exists on GitHub and resolves (annotated tags peeled) to
#     BUILD_SHA. This script never creates or moves a tag.
# Artifacts: one of them is SHA256SUMS; every other artifact except the
#   package summary must be listed in it and match it.
# Release state, read from the complete release list of the repository:
#   - No release for TAG: create a draft (`--verify-tag`), then confirm
#     exactly one draft exists.
#   - Exactly one draft for TAG: replace its assets and notes, re-checking
#     before and after every mutation that it is still a draft.
#   - A published release for TAG, or more than one release: refuse; nothing
#     is uploaded, edited, deleted or switched back to draft.
#   - Any `gh` failure (network, authentication, rate limit, server error)
#     stops the script; it is never read as "release does not exist".
#
# Usage: publish_draft_release.sh --tag TAG --sha BUILD_SHA --version VERSION
#          --summary PACKAGE_SUMMARY --notes NOTES_FILE ARTIFACT...
# Environment: GH_REPO (owner/name) and GH_TOKEN for `gh`.
set -euo pipefail

fail() {
    echo "::error::publish_draft_release: $*" >&2
    exit 1
}

tag="" sha="" version="" summary="" notes=""
artifacts=()
while [ "$#" -gt 0 ]; do
    case "$1" in
        --tag) tag=$2; shift 2 ;;
        --sha) sha=$2; shift 2 ;;
        --version) version=$2; shift 2 ;;
        --summary) summary=$2; shift 2 ;;
        --notes) notes=$2; shift 2 ;;
        --*) fail "unknown option $1" ;;
        *) artifacts+=("$1"); shift ;;
    esac
done

[ -n "${GH_REPO:-}" ] || fail "GH_REPO is not set"
[[ "$sha" =~ ^[0-9a-f]{40}$ ]] || fail "build commit must be a full SHA-1, got '$sha'"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$ ]] \
    || fail "invalid version '$version'"
[ "$tag" = "v$version" ] || fail "tag '$tag' does not name version $version (expected v$version)"
[ -f "$notes" ] || fail "release notes file missing: $notes"
[ "${#artifacts[@]}" -gt 0 ] || fail "no artifacts given"
for artifact in "${artifacts[@]}"; do
    [ -s "$artifact" ] || fail "artifact missing or empty: $artifact"
done

# Package metadata.
[ -f "$summary" ] || fail "package summary missing: $summary"
summary_value() {
    awk -F= -v key="$1" '$1 == key { sub(/^[^=]*=/, ""); print; exit }' "$summary"
}
[ "$(summary_value version)" = "$version" ] \
    || fail "package summary version '$(summary_value version)' is not $version"
[ "$(summary_value git_revision)" = "$sha" ] \
    || fail "package summary git_revision '$(summary_value git_revision)' is not the build commit $sha"

# Crate versions of the checked-out build commit: the app and the CLI are
# both shipped, and the packaging script reads the CLI's version.
for crate in d4lc-app d4lc-cli; do
    crate_version="$(awk -F'"' '/^version = / { print $2; exit }' "crates/$crate/Cargo.toml")"
    [ "$crate_version" = "$version" ] \
        || fail "$crate crate version '$crate_version' is not $version"
done

# Checksums of the artifacts about to be uploaded.
sums=""
to_verify=()
for artifact in "${artifacts[@]}"; do
    if [ "$(basename "$artifact")" = SHA256SUMS ]; then
        [ -z "$sums" ] || fail "more than one SHA256SUMS given"
        sums=$artifact
    elif ! [ "$artifact" -ef "$summary" ]; then
        to_verify+=("$artifact")
    fi
done
[ -n "$sums" ] || fail "no SHA256SUMS among the artifacts"
[ "${#to_verify[@]}" -gt 0 ] || fail "no release artifact to verify against SHA256SUMS"
"$(dirname "${BASH_SOURCE[0]}")/verify_release_checksums.sh" "$sums" "${to_verify[@]}" \
    || fail "artifact checksums do not match SHA256SUMS"

# Tag → commit, peeling annotated tags.
ref_json="$(gh api "repos/$GH_REPO/git/ref/tags/$tag")" \
    || fail "tag $tag must exist on GitHub before publishing (lookup failed)"
object_type="$(jq -r '.object.type' <<<"$ref_json")"
object_sha="$(jq -r '.object.sha' <<<"$ref_json")"
for _ in 1 2 3 4 5; do
    [ "$object_type" = "tag" ] || break
    tag_json="$(gh api "repos/$GH_REPO/git/tags/$object_sha")" \
        || fail "cannot read annotated tag object $object_sha"
    object_type="$(jq -r '.object.type' <<<"$tag_json")"
    object_sha="$(jq -r '.object.sha' <<<"$tag_json")"
done
[ "$object_type" = "commit" ] || fail "tag $tag does not resolve to a commit (got '$object_type')"
[ "$object_sha" = "$sha" ] \
    || fail "tag $tag points to $object_sha, but the artifacts were built from $sha"

# Release state. The complete release list (drafts included; the token has
# contents: write) is the only proof that no release exists for the tag. Any
# failure to read it (network, authentication, rate limit, server error, a
# partial page) stops the script; nothing is inferred from an error text.
release_entries() {
    # One "<id> <draft>" line per release whose tag_name is $tag. The tag is
    # validated above (v<semver>), so it is safe inside the jq string.
    gh api --paginate "repos/$GH_REPO/releases?per_page=100" \
        --jq ".[] | select(.tag_name == \"$tag\") | \"\(.id) \(.draft)\""
}
entries="$(release_entries)" \
    || fail "could not list the releases of $GH_REPO; the state of $tag is unknown and nothing was changed"
count="$(printf '%s' "$entries" | grep -c . || true)"

# Re-read one release right before (and after) a mutation: a draft that was
# published in between is never modified further.
require_draft() {
    local state
    state="$(gh api "repos/$GH_REPO/releases/$1" --jq '"\(.draft) \(.tag_name)"')" \
        || fail "could not re-read release $1 for $tag; stopping"
    [ "$state" = "true $tag" ] \
        || fail "release $1 for $tag is no longer a draft of this tag (state: $state); stopping"
}

# SemVer prereleases stay off GitHub's stable/Latest channel, including on refresh.
release_flags=(--prerelease=false)
if [[ "${version%%+*}" == *-* ]]; then
    release_flags=(--prerelease=true --latest=false)
fi

case "$count" in
    0)
        echo "Creating draft release $tag (build $sha)"
        gh release create "$tag" "${artifacts[@]}" \
            --draft \
            --verify-tag \
            --title "$tag" \
            --notes-file "$notes" "${release_flags[@]}"
        entries="$(release_entries)" \
            || fail "created a draft for $tag but could not list the releases to confirm it"
        [ "$(printf '%s' "$entries" | grep -c . || true)" = 1 ] \
            || fail "after creating the draft, $tag has releases: ${entries//$'\n'/; }"
        require_draft "${entries%% *}"
        ;;
    1)
        release_id="${entries%% *}"
        is_draft="${entries#* }"
        [[ "$release_id" =~ ^[0-9]+$ ]] || fail "unexpected release entry for $tag: '$entries'"
        case "$is_draft" in
            false) fail "release $tag is already published; it is never modified by this workflow" ;;
            true) ;;
            *) fail "unexpected draft state for $tag: '$entries'" ;;
        esac
        echo "Refreshing draft release $tag (id $release_id, build $sha)"
        require_draft "$release_id"
        gh release upload "$tag" "${artifacts[@]}" --clobber
        require_draft "$release_id"
        gh release edit "$tag" --title "$tag" --notes-file "$notes" "${release_flags[@]}"
        require_draft "$release_id"
        ;;
    *)
        fail "$count releases exist for $tag (${entries//$'\n'/; }); resolve that by hand, nothing was changed"
        ;;
esac
