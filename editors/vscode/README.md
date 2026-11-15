# changewalk for VS Code and Cursor

A thin client that opens a changewalk review in VS Code or Cursor. The
`changewalk` CLI picks the change, checks its branch out, and launches the
editor; this extension renders the review.

One extension serves both editors. Cursor is a fork of VS Code and runs the
same `.vsix`. The only differences are the launch scheme (`vscode://` or
`cursor://`, both handled here) and the registry used to publish.

## Status

At parity with the Zed review flow, through `glab`:

- An ordered multi-file diff of the change, files in the model's reading
  order, with the description, summary, reading order and notes beside it.
- The model's notes sit on their lines as read-only comment threads.
- The merge request's own threads are drawn on their lines (replies,
  resolve/reopen, edit your own note); threads on removed lines go on the
  diff's left side. Threads on the change itself sit under the description.
- Comment on any changed line to draft a review comment; **changewalk: Post
  the review** places each draft on the merge request's head commit, posts an
  optional summary, and gives a verdict (approve / request changes). A
  comment GitLab cannot place is reported and the rest still post; a verdict
  waits while any comment is unplaced.
- Approvals, your reviewer standing, and related issues are shown.
- **Mark as reviewed** records the tip, so a later visit can show only what
  changed since. Pictures in the description that are project uploads are
  fetched through `glab` and drawn inline.

The reading order and notes arrive in the hand-off's `review` field, which
the CLI fills from the same order cache the Zed fork uses. Threads,
approvals and verdicts need a GitLab merge request and the `glab` CLI logged
in (`glab auth login`); a GitHub change opens as a plain ordered diff.

## How it is launched

The CLI opens:

```
vscode://changewalk.changewalk/review?repo=<clone>&base=<ref>&mr=<hand-off.json>
```

- `mr` — path to the hand-off JSON the CLI wrote (the change in GitLab's shape).
- `repo` — the clone the change is checked out in.
- `base` — what to diff against; the CLI computes it for stacked changes.

To try it without the CLI, run **changewalk: Open a review from a hand-off
file** from the Command Palette and give it a hand-off JSON path.

## Develop

```
npm install
npm run build        # or: npm run watch
npm run typecheck
```

Press F5 in VS Code to launch an Extension Development Host. Package a
`.vsix` with `npm run package` (needs `@vscode/vsce`); publish to Open VSX
for Cursor with `npm run publish:ovsx` (needs `ovsx`).
