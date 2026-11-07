# changewalk

Walk a GitLab merge request in Zed. Pick one you review, and its diff opens
with a model-suggested reading order, guiding notes, the merge request
description above the files, and the review threads under their lines.

```
changewalk ~/Repos/my-project
```

Arrow keys move, Enter checks the branch out and opens Zed while the list stays
open, `g` opens the merge request in GitLab, `r` refreshes the list from GitLab,
`u` and `d` scroll the description, `q` quits. A refresh also runs `git fetch --prune origin` in each clone, so
the branches Zed diffs against are current. The merge request most
recently opened in Zed is marked with `●`, and the footer says when. The
Approved column shows `✓ you`, `✓ you +1`, or `✓ 2`, and the description pane
names the approvers. The Diff column shows lines added and removed, and
Comments the number of comments with how many are yours. Stacked
merge requests sit under the one they are stacked on and are diffed against
that branch, not the default branch.

The list also carries the merge requests you opened yourself, drafts included:
the author reads `you`, and a draft is marked `Draft ·` before its title.

A refresh updates the screen in stages: the lists first, then approvals and
counts from GitLab, then line counts from the clone after a `git fetch`. Merge
requests GitLab has not updated since the last refresh are not asked about
again, and a fetch from the last two minutes is reused. Last, with
`ANTHROPIC_API_KEY` set, the reading order of the top three merge requests you
review is written to Zed's cache, so opening one shows its order at once;
`CHANGEWALK_ORDER_MODEL` and `CHANGEWALK_NOTES_MODEL` choose the models.

The last list shows at once from a cache and only changes when you press `r`,
except on a first run with nothing cached, when it fetches straight away.

## Requirements

- `glab`, logged in with `glab auth login`, or a `GITLAB_TOKEN` exported in the environment.
- A Zed build with the review changes, as an app bundle. Set `ZED_APP` or
  `--zed-app` to its path; the default is `~/Applications/Zed Dev.app`.
- A model for Zed: an `ANTHROPIC_API_KEY` in the shell, passed to Zed when it is
  launched, or a key saved in Zed's agent settings.

## Install

```
cargo build --release
ln -s "$PWD/target/release/changewalk" ~/.local/bin/changewalk
```

Set `CHANGEWALK_REPOS` to a colon-separated list of repositories to list from
any directory. Pass `--assignee` to list merge requests assigned to you instead
of ones you review.

## How it hands over to Zed

On Enter the tool fetches every remote branch, checks the merge request's branch
out, writes the merge request as JSON under the temporary directory, and opens
`zed://git/review?repo=…&base=…&mr=…`. Zed opens the branch diff against the
base, shows the merge request from the file, and asks the model for an order and
notes once the files have loaded.
