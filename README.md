# changewalk

Walk a merge or pull request in Zed. Pick one you review, and its diff opens
with a model-suggested reading order, guiding notes, the description above the
files, and the review threads under their lines. GitLab and GitHub both work,
and a clone of each can sit in the same list.

```
changewalk ~/Repos/my-project
```

Arrow keys move, Enter checks the branch out and opens Zed while the list stays
open, `g` opens the change on its host, `r` refreshes the list, `u` and `d`
scroll the description, `q` quits. A refresh also runs `git fetch --prune
origin` in each clone, so the branches Zed diffs against are current. The
change most recently opened in Zed is marked with `●`, and the footer says
when. The Approved column shows `✓ you`, `✓ you +1`, or `✓ 2`, and the
description pane names the approvers. The Diff column shows lines added and
removed, and Comments the number of comments with how many are yours. Stacked
changes sit under the one they are stacked on and are diffed against that
branch, not the default branch.

Each host keeps its own words: a GitLab list is headed "Merge requests to
review" and numbered `!1628`, a GitHub one "Pull requests to review" and
`#1628`.

The list also carries the changes you opened yourself, drafts included: the
author reads `you`, and a draft is marked `Draft ·` before its title.

A refresh updates the screen in stages: the lists first, then approvals and
counts from the host, then line counts from the clone after a `git fetch`.
Changes the host has not updated since the last refresh are not asked about
again, and a fetch from the last two minutes is reused. Last, with
`ANTHROPIC_API_KEY` set, the reading order of the top three changes you review
is written to Zed's cache, so opening one shows its order at once;
`CHANGEWALK_ORDER_MODEL` and `CHANGEWALK_NOTES_MODEL` choose the models.

The last list shows at once from a cache and only changes when you press `r`,
except on a first run with nothing cached, when it fetches straight away.

## First run

The first run asks two questions: which host you review on, and which editor to
read reviews in. The answers go in `~/.config/changewalk/config.json`;
`changewalk --onboard` asks again.

The host you pick is only the assumption for a clone whose remote names neither
host, as a self-hosted GitLab or a GitHub Enterprise install does not. A remote
that says `gitlab` or `github` is read as that host whichever you picked, so a
mixed set of clones needs no further thought.

Zed is the only editor that can show a review so far, so there is nothing else
to pick yet. The choice is stored anyway, along with the app bundle onboarding
found, so a second editor is a matter of listing it.

## Requirements

- For GitLab: `glab`, logged in (`glab auth login`), or a `GITLAB_TOKEN` in the
  environment.
- For GitHub: `gh`, logged in (`gh auth login`), or a `GITHUB_TOKEN` or
  `GH_TOKEN` in the environment. A GitHub Enterprise host is reached at
  `https://<host>/api/v3`.
- A Zed build with the review changes, as an app bundle. Onboarding looks for
  `Zed Dev.app` and then `Zed.app` under `~/Applications` and `/Applications`;
  `ZED_APP` or `--zed-app` overrides what it found.
- A model for Zed: an `ANTHROPIC_API_KEY` in the shell, passed to Zed when it is
  launched, or a key saved in Zed's agent settings.

## Install

```
cargo build --release
ln -s "$PWD/target/release/changewalk" ~/.local/bin/changewalk
```

Set `CHANGEWALK_REPOS` to a colon-separated list of repositories to list from
any directory. Pass `--assignee` to list changes assigned to you instead of
ones you review.

## How it hands over to Zed

On Enter the tool fetches every remote branch, checks the change's branch out,
writes the change as JSON under the temporary directory, and opens
`zed://git/review?repo=…&base=…&mr=…`. Zed opens the branch diff against the
base, shows the change from the file, and asks the model for an order and notes
once the files have loaded.

The file is written in GitLab's shape, which is what Zed reads: a pull request
goes in the same fields, its number as the `iid`, `head` and `base` as the
source and target branches. Zed needs to know nothing about which host the
change came from.

## How each host is read

GitLab answers in one request per role — the merge requests you review or are
assigned, and the ones you authored — and one GraphQL query per project for
approvals, line counts, and comment counts.

GitHub needs a little more care. It drops you from a pull request's reviewers
the moment you submit a review, and a review half-done is exactly the one worth
listing, so which pull requests to show comes from search (`review-requested:`,
`reviewed-by:`, `author:`) rather than from the reviewers on the pull request.
The bodies then come from one listing of the open pull requests, with a single
request for any wanted number the first hundred did not reach. Approval is not
a flag either but the last of a reviewer's reviews, so a `CHANGES_REQUESTED` or
`DISMISSED` after an `APPROVED` takes it back.
