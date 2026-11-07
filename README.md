<div align="center">

# changewalk

**Walk a GitLab or GitHub review in Zed — picked, ordered, and annotated.**

[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584.svg)](https://www.rust-lang.org)

<!--
  Demo: replace the line below with a GIF of the picker.
  Record with `vhs` (https://github.com/charmbracelet/vhs) or
  `asciinema` + `agg`, and save it to docs/demo.gif.
-->
<!-- ![changewalk picker](docs/demo.gif) -->

</div>

Pick a merge or pull request you review, and its diff opens in Zed with a
model-suggested reading order, guiding notes, the description above the files,
and the review threads under their lines. GitLab and GitHub both work, and a
clone of each can sit in the same list.

## Features

- Review GitLab **merge requests** and GitHub **pull requests** from one list,
  with a clone of each host mixed in.
- A **model-suggested reading order** and guiding notes, written to Zed's cache
  so a change opens in order at once.
- **Approvals, diff line counts, and comment counts** shown in the list, with
  the approvers named in the description pane.
- **Stacked changes** sit under the one they are stacked on, and are diffed
  against that branch rather than the default branch.
- Your **own changes**, drafts included, listed alongside the ones you review.
- Each host keeps its own words: a GitLab list is headed "Merge requests to
  review" and numbered `!1628`, a GitHub one "Pull requests to review" and
  `#1628`.

## Quickstart

```
cargo build --release
ln -s "$PWD/target/release/changewalk" ~/.local/bin/changewalk

changewalk ~/Repos/my-project
```

The first run asks two questions and stores the answers (see
[Configuration](#configuration)). Set `CHANGEWALK_REPOS` to a colon-separated
list of repositories to list from any directory. Pass `--assignee` to list
changes assigned to you instead of ones you review.

## Keybindings

| Key | Action |
| --- | --- |
| `↑` `↓` | Move through the list |
| `Enter` | Check the branch out and open Zed (the list stays open) |
| `g` | Open the change on its host |
| `r` | Refresh the list |
| `u` `d` | Scroll the description |
| `q` | Quit |

A refresh also runs `git fetch --prune origin` in each clone, so the branches
Zed diffs against are current. The change most recently opened in Zed is marked
with `●`, and the footer says when. The Approved column shows `✓ you`,
`✓ you +1`, or `✓ 2`. The Diff column shows lines added and removed, and
Comments the number of comments with how many are yours. A draft is marked
`Draft ·` before its title, and a change you authored reads `you`.

A refresh updates the screen in stages: the lists first, then approvals and
counts from the host, then line counts from the clone after a `git fetch`.
Changes the host has not updated since the last refresh are not asked about
again, and a fetch from the last two minutes is reused. Last, with
`ANTHROPIC_API_KEY` set, the reading order of the top three changes you review
is written to Zed's cache, so opening one shows its order at once;
`CHANGEWALK_ORDER_MODEL` and `CHANGEWALK_NOTES_MODEL` choose the models.

The last list shows at once from a cache and only changes when you press `r`,
except on a first run with nothing cached, when it fetches straight away.

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

## Configuration

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

### Environment

| Variable | Purpose |
| --- | --- |
| `CHANGEWALK_REPOS` | Colon-separated repositories to list from any directory |
| `ANTHROPIC_API_KEY` | Enables reading-order and notes; passed to Zed on launch |
| `CHANGEWALK_ORDER_MODEL` | Model for the reading order |
| `CHANGEWALK_NOTES_MODEL` | Model for the guiding notes |
| `ZED_APP` | Path to the Zed app bundle (overrides what onboarding found) |
| `GITLAB_TOKEN` | GitLab token, if not using `glab` |
| `GITHUB_TOKEN` / `GH_TOKEN` | GitHub token, if not using `gh` |

### Starting over

`changewalk --reset-config` forgets the host and editor so the next run
onboards afresh, and `changewalk --reset-cache` clears the cached lists,
usernames, and last-opened marker so the next run fetches fresh. The two are
independent and can be given together. Each lists what it will delete and asks
before doing it; `--yes` (or `-y`) answers for you, and with no terminal to ask
in nothing is deleted unless `--yes` is passed.

## How it works

### Handing over to Zed

On Enter the tool fetches every remote branch, checks the change's branch out,
writes the change as JSON under the temporary directory, and opens
`zed://git/review?repo=…&base=…&mr=…`. Zed opens the branch diff against the
base, shows the change from the file, and asks the model for an order and notes
once the files have loaded.

The file is written in GitLab's shape, which is what Zed reads: a pull request
goes in the same fields, its number as the `iid`, `head` and `base` as the
source and target branches. Zed needs to know nothing about which host the
change came from.

### Reading each host

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

## Licence

[MIT](LICENSE)
