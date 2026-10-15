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

## Requirements

- `python3` (uses only the standard library)
- `glab`, logged in (`glab auth login`)
- A Zed build with the review changes, as an app bundle. Set `ZED_APP` to its
  path; the default is `~/Applications/Zed Dev.app`.
- An `ANTHROPIC_API_KEY` in the shell, passed to Zed when it is launched, or a
  key saved in Zed's agent settings.

## Install

```
ln -s "$PWD/zed-review" ~/.local/bin/zed-review
```

Set `ZED_REVIEW_REPOS` to a colon-separated list of repositories to list from
any directory. Pass `--assignee` to list merge requests assigned to you instead
of ones you review.
