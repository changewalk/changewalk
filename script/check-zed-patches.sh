#!/usr/bin/env bash
#
# check-zed-patches.sh
#
# Verify that the Zed review-flow patch series (editors/zed-patches/*.patch)
# still applies cleanly onto upstream Zed, WITHOUT building Zed.
#
# This is the cheap half of build-zed.sh: fetch upstream, `git am` the series,
# stop. It never compiles anything, so it runs in seconds.
#
# It fetches as little of Zed as possible, fresh each run:
#
#   * --depth 1          one commit, not Zed's multi-GB history.
#   * --filter=blob:none blobs are fetched lazily, only when checked out.
#   * sparse-checkout    only the directories the patches actually touch,
#                        derived automatically from the patch files. So the
#                        blobs that download are just those few crates, not
#                        the whole Zed source tree.
#
# Usage:
#   check-zed-patches.sh            # apply onto the pinned ref (editors/zed-patches/UPSTREAM)
#   check-zed-patches.sh <ref>      # apply onto another ref (tag/branch), for local checks
#
# Environment overrides:
#   ZED_BUILD_DIR     where to clone Zed   (default ~/.cache/changewalk/zed-check)
#   ZED_UPSTREAM_URL  upstream Zed remote  (default https://github.com/zed-industries/zed.git)
#
# Exit status: 0 if the whole series applies, non-zero otherwise.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PATCH_DIR="$REPO_DIR/editors/zed-patches"

UPSTREAM_URL="${ZED_UPSTREAM_URL:-https://github.com/zed-industries/zed.git}"
BUILD_DIR="${ZED_BUILD_DIR:-$HOME/.cache/changewalk/zed-check}"

die() { echo "ERROR: $1" >&2; exit 1; }

command -v git >/dev/null || die "git not found on PATH."

[ -f "$PATCH_DIR/UPSTREAM" ] || die "missing $PATCH_DIR/UPSTREAM"
PINNED="$(tr -d ' \t\r\n' < "$PATCH_DIR/UPSTREAM")"
[ -n "$PINNED" ] || die "editors/zed-patches/UPSTREAM is empty"

# Target ref: the argument, else the pinned ref.
REF="${1:-$PINNED}"

shopt -s nullglob
PATCHES=("$PATCH_DIR"/*.patch)
shopt -u nullglob
[ "${#PATCHES[@]}" -gt 0 ] || die "no patches found in $PATCH_DIR"

# --- directories the patches touch -----------------------------------------
# Parse the `diff --git a/<path> b/<path>` headers, take each file's directory,
# and keep the unique set (dropping "." for root-level files, which cone-mode
# sparse-checkout always includes anyway). This is what we ask Zed for, so the
# set stays in step with the patches with zero manual upkeep.
mapfile -t SPARSE_DIRS < <(
    grep -h '^diff --git ' "${PATCHES[@]}" \
        | sed -E 's#^diff --git a/##; s# b/.*##' \
        | xargs -n1 dirname \
        | grep -v '^\.$' \
        | sort -u
)
[ "${#SPARSE_DIRS[@]}" -gt 0 ] || die "could not determine any paths from the patches"

echo "==> Target ref   : $REF"
echo "==> Build dir    : $BUILD_DIR"
echo "==> Patches      : ${#PATCHES[@]}"
echo "==> Sparse dirs  : ${#SPARSE_DIRS[@]} (${SPARSE_DIRS[*]})"

# --- fresh, minimal clone --------------------------------------------------
rm -rf "$BUILD_DIR"
mkdir -p "$BUILD_DIR"
git -C "$BUILD_DIR" init --quiet
git -C "$BUILD_DIR" remote add origin "$UPSTREAM_URL"
# Mark origin as a promisor remote so the blobless fetch below is allowed and
# any missing blob is lazily backfilled on demand.
git -C "$BUILD_DIR" config remote.origin.promisor true
git -C "$BUILD_DIR" config remote.origin.partialclonefilter blob:none
# Restrict the working tree to just the touched directories BEFORE checkout, so
# only those blobs are ever fetched.
git -C "$BUILD_DIR" sparse-checkout init --cone
git -C "$BUILD_DIR" sparse-checkout set "${SPARSE_DIRS[@]}"

echo "==> Fetching $REF (depth 1, blobless) ..."
git -C "$BUILD_DIR" fetch --quiet --depth 1 --filter=blob:none --tags --force \
    origin "$REF" \
    || die "cannot fetch upstream ref '$REF' from $UPSTREAM_URL"

git -C "$BUILD_DIR" checkout --quiet --detach FETCH_HEAD

echo "==> Applying patch series onto $(git -C "$BUILD_DIR" rev-parse --short HEAD) ..."
if git -C "$BUILD_DIR" am --3way "${PATCHES[@]}"; then
    echo "==> OK: all ${#PATCHES[@]} patches apply cleanly onto '$REF'."
    STATUS=0
else
    git -C "$BUILD_DIR" am --abort 2>/dev/null || true
    echo "ERROR: patch series does NOT apply cleanly onto '$REF'." >&2
    echo "       Refresh the series against upstream with script/sync-zed.sh" >&2
    echo "       (resolve the conflicts, then re-export the patches)." >&2
    STATUS=1
fi

rm -rf "$BUILD_DIR"
exit "$STATUS"
