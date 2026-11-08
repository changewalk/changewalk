#!/usr/bin/env bash
#
# build-zed.sh
#
# Build the Zed app that changewalk drives, from this repository alone.
#
# There is no separate Zed fork. The review-flow changes live here as a patch
# series under editors/zed-patches/. This script:
#
#   1. Reads the pinned upstream ref from editors/zed-patches/UPSTREAM.
#   2. Clones upstream Zed (zed-industries/zed) at that ref into a build dir
#      outside this repository (reused between runs).
#   3. Applies editors/zed-patches/*.patch on top with `git am`.
#   4. Builds the release bundle with upstream Zed's bundler for this OS
#      (bundle-mac / bundle-linux / bundle-freebsd).
#
# The build dir is self-contained, so once a build works you can delete the
# old ~/Repos/changewalk/zed fork entirely.
#
# Prerequisites:
#   - git and a Rust toolchain (rustup/cargo) on PATH.
#   - macOS: Xcode and its Metal toolchain. If the build fails with
#     "missing Metal Toolchain", run once:
#         xcodebuild -downloadComponent MetalToolchain
#   - Linux: upstream's build deps (run script/linux in the build dir once).
#   On macOS the build also installs to /Applications (bundle-mac -i, enabled
#   by patch 0004). On Linux/FreeBSD the bundle is written under the build dir
#   and you install it yourself; the path is printed at the end.
#
# Environment overrides:
#   ZED_BUILD_DIR     where to clone/build Zed   (default ~/.cache/changewalk/zed-build)
#   ZED_UPSTREAM_URL  upstream Zed remote        (default https://github.com/zed-industries/zed.git)
#   ZED_FORK_REPO     optional local Zed clone used only to speed the first
#                     clone (default ~/Repos/changewalk/zed; ignored if absent)
#
# Usage:
#   script/build-zed.sh              # build (and, on macOS, install)
#   script/build-zed.sh -- <args>    # forward <args> to the OS bundler instead

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PATCH_DIR="$REPO_DIR/editors/zed-patches"

UPSTREAM_URL="${ZED_UPSTREAM_URL:-https://github.com/zed-industries/zed.git}"
BUILD_DIR="${ZED_BUILD_DIR:-$HOME/.cache/changewalk/zed-build}"
FORK_REPO="${ZED_FORK_REPO:-$HOME/Repos/changewalk/zed}"

die() { echo "ERROR: $1" >&2; exit 1; }

# --- pick the bundler for this OS ------------------------------------------
# Default args are per-OS: macOS installs with -i (patch 0004); the Linux and
# FreeBSD bundlers take no such flag and just produce a package.
case "$(uname -s)" in
    Darwin)  BUNDLER="script/bundle-mac";     DEFAULT_ARGS=(-i) ;;
    Linux)   BUNDLER="script/bundle-linux";   DEFAULT_ARGS=() ;;
    FreeBSD) BUNDLER="script/bundle-freebsd"; DEFAULT_ARGS=() ;;
    *) die "unsupported OS '$(uname -s)'. Build upstream Zed manually in the
       build dir (on Windows use script/bundle-windows.ps1)." ;;
esac

# Bundler arguments: everything after a literal `--`, else the per-OS default.
# The `[@]+...` guard keeps empty arrays safe under `set -u` on bash 3.2 (macOS).
BUNDLE_ARGS=("${DEFAULT_ARGS[@]+"${DEFAULT_ARGS[@]}"}")
if [ "${1:-}" = "--" ]; then
    shift
    BUNDLE_ARGS=("$@")
fi

command -v git >/dev/null   || die "git not found on PATH."
command -v cargo >/dev/null || die "cargo (Rust toolchain) not found on PATH; install from https://rustup.rs"

[ -f "$PATCH_DIR/UPSTREAM" ] || die "missing $PATCH_DIR/UPSTREAM"
REF="$(tr -d ' \t\r\n' < "$PATCH_DIR/UPSTREAM")"
[ -n "$REF" ] || die "editors/zed-patches/UPSTREAM is empty"

shopt -s nullglob
PATCHES=("$PATCH_DIR"/*.patch)
shopt -u nullglob
[ "${#PATCHES[@]}" -gt 0 ] || die "no patches found in $PATCH_DIR"

echo "==> Upstream ref : $REF"
echo "==> Build dir    : $BUILD_DIR"
echo "==> Patches      : ${#PATCHES[@]}"

# --- 1. make sure the build dir is a clone of upstream ----------------------
if [ ! -d "$BUILD_DIR/.git" ]; then
    echo "==> Cloning upstream Zed into $BUILD_DIR ..."
    mkdir -p "$(dirname "$BUILD_DIR")"
    REF_ARGS=()
    # Use the existing local fork only to avoid re-downloading ~400MB on the
    # first clone; --dissociate copies the objects so the build dir does not
    # depend on the fork afterwards.
    if [ -d "$FORK_REPO/.git" ]; then
        REF_ARGS=(--reference-if-able "$FORK_REPO" --dissociate)
    fi
    git clone "${REF_ARGS[@]}" "$UPSTREAM_URL" "$BUILD_DIR"
fi

cd "$BUILD_DIR"

# Point origin at upstream in case ZED_UPSTREAM_URL changed.
git remote set-url origin "$UPSTREAM_URL"

# --- 2. check out the pinned ref, clean ------------------------------------
echo "==> Fetching $REF ..."
# Fetch the ref whether it is a tag, branch, or raw SHA.
git fetch --tags --force origin "+$REF:refs/changewalk/upstream" 2>/dev/null \
    || git fetch --tags --force origin
RESOLVED="$(git rev-parse --verify --quiet refs/changewalk/upstream 2>/dev/null \
    || git rev-parse --verify "$REF")" \
    || die "cannot resolve upstream ref '$REF'"

echo "==> Resetting build tree to $REF ($(git rev-parse --short "$RESOLVED")) ..."
git am --abort 2>/dev/null || true
git checkout --quiet --detach "$RESOLVED"
git reset --hard --quiet "$RESOLVED"
git clean -ffdq

# --- 3. apply the patch series ---------------------------------------------
echo "==> Applying patch series ..."
if ! git am --3way "${PATCHES[@]}"; then
    git am --abort 2>/dev/null || true
    die "patches do not apply cleanly onto '$REF'. Refresh them against this
       ref with script/sync-zed.sh (resolve conflicts, then re-export)."
fi
echo "==> Patches applied. Build tree at $(git rev-parse --short HEAD)."

# --- 4. build + install -----------------------------------------------------
[ -x "$BUNDLER" ] || die "upstream bundler '$BUNDLER' not found in the build dir."
echo "==> Building: $BUNDLER ${BUNDLE_ARGS[*]+${BUNDLE_ARGS[*]}}"
"./$BUNDLER" "${BUNDLE_ARGS[@]+"${BUNDLE_ARGS[@]}"}"

echo "==> Done."
