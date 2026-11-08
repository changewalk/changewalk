#!/usr/bin/env bash
#
# sync-zed.sh
#
# Move the review-flow patch series onto a newer upstream Zed, replacing the
# weekly "rebase the fork" step. It re-applies editors/zed-patches/*.patch onto a new
# upstream ref and, when they apply, re-exports the patches and repins
# editors/zed-patches/UPSTREAM.
#
#   script/sync-zed.sh <new-ref>
#       Apply the patches onto <new-ref> (a tag, branch, or SHA) in the build
#       dir. If they apply cleanly, re-export the patches and set UPSTREAM to
#       <new-ref>. If they conflict, the build dir is left mid-`git am` so you
#       can resolve it:
#
#           cd "$ZED_BUILD_DIR" (default ~/.cache/changewalk/zed-build)
#           # fix the conflicted files, then:
#           git am --continue        # repeat until the series is applied
#           # back here:
#           script/sync-zed.sh --export <new-ref>
#
#   script/sync-zed.sh --export <new-ref>
#       Re-export the build dir's current commits (the applied series) back
#       into editors/zed-patches/ and set UPSTREAM to <new-ref>. Use after resolving a
#       conflicted apply by hand.
#
# After a successful sync, run script/build-zed.sh to build and install, and
# commit the updated editors/zed-patches/.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PATCH_DIR="$REPO_DIR/editors/zed-patches"

UPSTREAM_URL="${ZED_UPSTREAM_URL:-https://github.com/zed-industries/zed.git}"
BUILD_DIR="${ZED_BUILD_DIR:-$HOME/.cache/changewalk/zed-build}"
FORK_REPO="${ZED_FORK_REPO:-$HOME/Repos/changewalk/zed}"

die() { echo "ERROR: $1" >&2; exit 1; }

EXPORT_ONLY=0
if [ "${1:-}" = "--export" ]; then
    EXPORT_ONLY=1
    shift
fi
NEW_REF="${1:-}"
[ -n "$NEW_REF" ] || die "usage: script/sync-zed.sh [--export] <new-ref>"

export_patches() {
    # Re-export <base>..HEAD from the build dir into editors/zed-patches/ and repin.
    local base="$1"
    echo "==> Re-exporting patches ($(git -C "$BUILD_DIR" rev-list --count "$base"..HEAD) commits) ..."
    rm -f "$PATCH_DIR"/*.patch
    git -C "$BUILD_DIR" format-patch "$base"..HEAD -o "$PATCH_DIR" >/dev/null
    printf '%s\n' "$NEW_REF" > "$PATCH_DIR/UPSTREAM"
    echo "==> editors/zed-patches/ refreshed and UPSTREAM pinned to $NEW_REF."
    echo "    Review, build with script/build-zed.sh, then commit editors/zed-patches/."
    ls "$PATCH_DIR"/*.patch
}

# --- ensure build dir exists ------------------------------------------------
if [ ! -d "$BUILD_DIR/.git" ]; then
    echo "==> Cloning upstream Zed into $BUILD_DIR ..."
    mkdir -p "$(dirname "$BUILD_DIR")"
    REF_ARGS=()
    [ -d "$FORK_REPO/.git" ] && REF_ARGS=(--reference-if-able "$FORK_REPO" --dissociate)
    git clone "${REF_ARGS[@]}" "$UPSTREAM_URL" "$BUILD_DIR"
fi
git -C "$BUILD_DIR" remote set-url origin "$UPSTREAM_URL"

echo "==> Fetching $NEW_REF ..."
git -C "$BUILD_DIR" fetch --tags --force origin "+$NEW_REF:refs/changewalk/upstream" 2>/dev/null \
    || git -C "$BUILD_DIR" fetch --tags --force origin
RESOLVED="$(git -C "$BUILD_DIR" rev-parse --verify --quiet refs/changewalk/upstream 2>/dev/null \
    || git -C "$BUILD_DIR" rev-parse --verify "$NEW_REF")" \
    || die "cannot resolve ref '$NEW_REF'"

# --- export-only path (after a hand-resolved git am) ------------------------
if [ "$EXPORT_ONLY" -eq 1 ]; then
    [ -d "$BUILD_DIR/.git/rebase-apply" ] && die "a git am is still in progress in $BUILD_DIR; finish it (git am --continue) first."
    export_patches "$RESOLVED"
    exit 0
fi

# --- apply the series onto the new ref -------------------------------------
shopt -s nullglob
PATCHES=("$PATCH_DIR"/*.patch)
shopt -u nullglob
[ "${#PATCHES[@]}" -gt 0 ] || die "no patches in $PATCH_DIR"

echo "==> Resetting build tree to $NEW_REF ($(git -C "$BUILD_DIR" rev-parse --short "$RESOLVED")) ..."
git -C "$BUILD_DIR" am --abort 2>/dev/null || true
git -C "$BUILD_DIR" checkout --quiet --detach "$RESOLVED"
git -C "$BUILD_DIR" reset --hard --quiet "$RESOLVED"
git -C "$BUILD_DIR" clean -ffdq

echo "==> Applying patch series onto $NEW_REF ..."
if git -C "$BUILD_DIR" am --3way "${PATCHES[@]}"; then
    export_patches "$RESOLVED"
else
    echo >&2
    echo "Patches conflict on $NEW_REF. The apply is paused in:" >&2
    echo "    $BUILD_DIR" >&2
    echo "Resolve each conflict, 'git am --continue' until done, then run:" >&2
    echo "    script/sync-zed.sh --export $NEW_REF" >&2
    exit 1
fi
