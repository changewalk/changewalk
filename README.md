<div align="center">

# changewalk

**Walk a GitLab or GitHub review in Zed, VS Code, or Cursor.**

[![Licence: MIT](https://img.shields.io/badge/licence-MIT-blue.svg)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584.svg)](https://www.rust-lang.org)

<!--
  Demo: replace the line below with a GIF of the picker.
  Record with `vhs` (https://github.com/charmbracelet/vhs) or
  `asciinema` + `agg`, and save it to docs/demo.gif.
-->
<!-- ![changewalk picker](docs/demo.gif) -->

</div>

Pick a merge or pull request you review, and its diff opens in your editor with
the description beside the files. GitLab and GitHub both work, and a clone of
each can sit in the same list.

Zed shows the most: a model-suggested reading order, guiding notes, and the
review threads under their lines. VS Code and Cursor open the change as an
ordered multi-file diff with its description for now, and the rest is on the way.

## Features

- Review GitLab **merge requests** and GitHub **pull requests** from one list,
  with a clone of each host mixed in.
- In **Zed**, a **model-suggested reading order** and guiding notes, so a change
  opens in order at once.
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
| `Enter` | Check the branch out and open your editor (the list stays open) |
| `g` | Open the change on its host |
| `r` | Refresh the list |
| `u` `d` | Scroll the description |
| `q` | Quit |

A refresh also runs `git fetch --prune origin` in each clone, so the branches
your editor diffs against are current. On Enter a local branch left from an
earlier review follows the remote, even after the author rebases, as long as
it is only an old copy of the remote; one you have committed on is left alone
and the footer says so. The change most recently opened is marked
with `●`, and the footer says when. The Approved column shows `✓ you`,
`✓ you +1`, or `✓ 2`. The Diff column shows lines added and removed, and
Comments the number of comments with how many are yours. A draft is marked
`Draft ·` before its title, and a change you authored reads `you`.

A refresh updates the screen in stages: the lists first, then approvals and
counts from the host, then line counts from the clone after a `git fetch`.
Changes the host has not updated since the last refresh are not asked about
again, and a fetch from the last two minutes is reused. Last, with
`ANTHROPIC_API_KEY` set and Zed as the editor, the reading order of the top
three changes you review is written to Zed's cache, so opening one shows its
order at once; `CHANGEWALK_ORDER_MODEL` and `CHANGEWALK_NOTES_MODEL` choose the
models.

The last list shows at once from a cache and only changes when you press `r`,
except on a first run with nothing cached, when it fetches straight away.

## Requirements

- For GitLab: `glab`, logged in (`glab auth login`), or a `GITLAB_TOKEN` in the
  environment.
- For GitHub: `gh`, logged in (`gh auth login`), or a `GITHUB_TOKEN` or
  `GH_TOKEN` in the environment. A GitHub Enterprise host is reached at
  `https://<host>/api/v3`.
- One of the editors below, installed as an app bundle. Onboarding finds it
  under `~/Applications` and `/Applications`; `--app` (or `ZED_APP`) overrides
  what it found.
- With Zed, a model for the reading order and notes: an `ANTHROPIC_API_KEY` in
  the shell, passed to Zed when it is launched, or a key saved in Zed's agent
  settings.

## Editors

A review opens in **Zed**, **VS Code**, or **Cursor**. Onboarding asks which,
and `changewalk --onboard` asks again; `--app` or the saved app-bundle path
overrides the one onboarding found.

Zed shows the whole review: the diff in reading order, guiding notes, the
description, and the review threads under their lines. VS Code and Cursor show
an ordered multi-file diff with the description beside it; the reading order,
notes, and threads are on their way to them from the CLI.

### Zed

The review changes live in a fork, not in upstream Zed:

- Repository: `git@github.com:changewalk/zed.git`
- Branch: `git-diff-branch-view`, rebased onto upstream Zed each week.

The fork is a sibling of this repository, as `../zed`. Build and install the app
from it:

```
git clone git@github.com:changewalk/zed.git ../zed
cd ../zed
git checkout git-diff-branch-view
script/bundle-mac -i   # installs to /Applications/Zed.app
```

### VS Code and Cursor

One extension serves both, since Cursor is a fork of VS Code. Build and install
it from this repository:

```
cd editors/vscode
npm install
npm run package                                # writes changewalk-<version>.vsix
code   --install-extension changewalk-*.vsix   # VS Code
cursor --install-extension changewalk-*.vsix   # Cursor
```

See [`editors/vscode/README.md`](editors/vscode/README.md) for more.

## Configuration

The first run asks two questions: which host you review on, and which editor to
read reviews in. The answers go in `~/.config/changewalk/config.json`;
`changewalk --onboard` asks again.

The host you pick is only the assumption for a clone whose remote names neither
host, as a self-hosted GitLab or a GitHub Enterprise install does not. A remote
that says `gitlab` or `github` is read as that host whichever you picked, so a
mixed set of clones needs no further thought.

The editor you pick — Zed, VS Code, or Cursor — is stored too, with the app
bundle onboarding found. `--app` (or `ZED_APP`) overrides the bundle, and
`--onboard` changes the editor. See [Editors](#editors) for what each shows.

### Environment

| Variable | Purpose |
| --- | --- |
| `CHANGEWALK_REPOS` | Colon-separated repositories to list from any directory |
| `ANTHROPIC_API_KEY` | Enables reading-order and notes in Zed; passed to Zed on launch |
| `CHANGEWALK_ORDER_MODEL` | Model for the reading order |
| `CHANGEWALK_NOTES_MODEL` | Model for the guiding notes |
| `ZED_APP` | Path to the editor app bundle, same as `--app` (overrides onboarding) |
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

### Handing over to the editor

On Enter the tool fetches every remote branch, checks the change's branch out,
writes the change as JSON under the temporary directory, and opens a review URL
under the editor's own scheme — `zed://git/review?…`,
`vscode://changewalk.changewalk/review?…`, or `cursor://…` — with the clone, the
base, and the JSON file as its query. The editor opens the branch diff against
the base and shows the change from the file; Zed also asks the model for an
order and notes once the files have loaded.

The file is written in GitLab's shape: a pull request goes in the same fields,
its number as the `iid`, its source and target branches as the branches. The
editor needs to know nothing about which host the change came from.

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
