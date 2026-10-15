# zed-review

Pick an open GitLab merge request you review, and open its diff in Zed with a
model-suggested reading order, guiding notes, and the merge request description
above the files.

```
zed-review ~/Repos/my-project
```

Arrow keys move, Enter checks the branch out and opens Zed, `o` opens the merge
request in the browser, `q` quits. Stacked merge requests sit under the one they
are stacked on and are diffed against that branch, not the default branch.

The last list shows at once from a cache and refreshes in the background.

## Requirements

- `glab`, logged in (`glab auth login`), or a `GITLAB_TOKEN` in the environment.
- A Zed build with the review changes, as an app bundle. Set `ZED_APP` or
  `--zed-app` to its path; the default is `~/Applications/Zed Dev.app`.
- A model for Zed: an `ANTHROPIC_API_KEY` in the shell, passed to Zed when it is
  launched, or a key saved in Zed's agent settings.

## Install

```
cargo build --release
ln -s "$PWD/target/release/zed-review" ~/.local/bin/zed-review
```

Set `ZED_REVIEW_REPOS` to a colon-separated list of repositories to list from
any directory. Pass `--assignee` to list merge requests assigned to you instead
of ones you review.

## How it hands over to Zed

On Enter the tool fetches the branch (and its base when stacked), checks it
out, writes the merge request as JSON under the temporary directory, and opens
`zed://git/review?repo=…&base=…&mr=…`. Zed opens the branch diff against the
base, shows the merge request from the file, and asks the model for an order and
notes once the files have loaded.
