# Zed patches

The review flow is not a Zed extension and cannot be one: it adds panels, a
multibuffer diff, review threads under lines, custom actions, and a
`zed://git/review?…` URL handler. Those touch Zed's `editor`, `git_ui`, and
`agent_ui` crates, which the WASM extension API does not expose. So the changes
must be compiled into the Zed app.

Rather than keep a whole Zed fork, this directory holds **only our changes** as
a patch series on top of a pinned upstream Zed. The app is built by cloning
upstream Zed at that ref and applying these patches.

## Files

- `UPSTREAM` — the upstream Zed ref the patches apply onto (a tag such as
  `v1.27.0`, or a branch/SHA).
- `NNNN-*.patch` — the custom commits, in order, from `git format-patch`.

## Prerequisites

- `git` and a Rust toolchain (`rustup`/`cargo`) on your `PATH`.
- **macOS:** Xcode and its Metal toolchain. If the build fails with
  "missing Metal Toolchain", run once: `xcodebuild -downloadComponent MetalToolchain`.
- **Linux:** upstream Zed's build deps — run `script/linux` inside the build
  dir once (`~/.cache/changewalk/zed-build`).

## Build and install the app

```sh
script/build-zed.sh          # clone upstream@UPSTREAM, apply patches, build
```

This clones upstream Zed into `~/.cache/changewalk/zed-build` (override with
`ZED_BUILD_DIR`), applies the patches, and runs upstream's bundler for your OS:

- **macOS** — `script/bundle-mac -i`, which installs `/Applications/Zed.app`
  (the `-i` install mode is what patch 0004 adds).
- **Linux / FreeBSD** — `script/bundle-linux` / `script/bundle-freebsd`, which
  write a package under the build dir for you to install; the path is printed
  at the end. (Windows: build with `script/bundle-windows.ps1` by hand.)

The build dir is self-contained. Pass bundler flags with
`script/build-zed.sh -- <args>`.

## Check the patches apply (no build)

```sh
script/check-zed-patches.sh                   # onto the pinned ref (UPSTREAM)
script/check-zed-patches.sh --latest-release  # onto the current Zed release (what CI does)
script/check-zed-patches.sh v1.28.0           # onto a specific ref
```

This is the cheap half of the build: it fetches a fresh, minimal slice of Zed
(one commit, and only the crates the patches touch) and runs `git am` to prove
the series still applies. It compiles nothing, so it finishes in seconds.

CI runs the `--latest-release` form on every push
(`.github/workflows/zed-patches.yml`): it asserts the patches keep applying
onto whatever the **current upstream Zed release** is — a deliberately moving
target — so the series can never silently fall behind. This means a commit that
was green can later go **red when Zed publishes a new release** that conflicts
with the patches, with no change to this repo. That red is the signal to
refresh the series against the new Zed with `script/sync-zed.sh` and bump
`UPSTREAM`. (The build itself still uses the pinned `UPSTREAM`, so the shipped
app stays reproducible.)

## Move onto a newer upstream Zed

```sh
script/sync-zed.sh v1.28.0   # or a branch/SHA, e.g. main
```

If the patches apply cleanly, they are re-exported and `UPSTREAM` is repinned.
If they conflict, the apply pauses in the build dir; resolve it, then:

```sh
cd ~/.cache/changewalk/zed-build
# fix conflicts, then: git am --continue   (repeat until the series is applied)
script/sync-zed.sh --export v1.28.0
```

Afterwards, run `script/build-zed.sh` and commit the updated `editors/zed-patches/`.

## Regenerating from the existing fork

These patches were exported from the `changewalk/zed` fork with:

```sh
git -C ../zed format-patch v1.27.0..git-diff-branch-view -o editors/zed-patches
```

Once a build from patches is confirmed working, the separate fork is no longer
needed.
