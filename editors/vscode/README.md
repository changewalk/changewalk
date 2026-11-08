# changewalk for VS Code and Cursor

A thin client that opens a changewalk review in VS Code or Cursor. The
`changewalk` CLI picks the change, checks its branch out, and launches the
editor; this extension renders the review.

One extension serves both editors. Cursor is a fork of VS Code and runs the
same `.vsix`. The only differences are the launch scheme (`vscode://` or
`cursor://`, both handled here) and the registry used to publish.

## Status

Phase 1. It opens an ordered multi-file diff of the change, with the
description beside it. Review threads, the model's reading order and notes,
and the summary come from the CLI's review payload in a later phase; the
renderer already orders files by `review.order` when the CLI sends it.

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
