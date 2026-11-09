// changewalk thin client for VS Code and Cursor.
//
// The CLI checks the change's branch out, writes it as JSON, and opens
//   vscode://changewalk.changewalk/review?repo=<dir>&base=<ref>&mr=<file>
// (Cursor uses the cursor:// scheme; the handler is the same). We read the
// hand-off and open an ordered multi-file diff of the change, with its
// description beside it.
//
// Phase 1 renders the diff and description. Review threads, the model's
// reading order, notes, and summary arrive in the hand-off's `review`
// payload in a later phase; the renderer already reads `review.order` to
// order the files, and ignores the rest until it is there.

import * as vscode from "vscode";
import * as path from "path";
import { Handoff, changeNumber, readHandoff } from "./handoff";
import { activateGitExtension } from "./git";
import {
  ReviewLaunch,
  changedFiles,
  fallbackBase,
  order,
  parseReviewUri,
  resolveBase,
} from "./review";

/** A URI scheme whose content is always empty, for the base side of an
 *  added file's diff. Works the same in VS Code and Cursor. */
const EMPTY_SCHEME = "changewalk-empty";

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(EMPTY_SCHEME, {
      provideTextDocumentContent: () => "",
    }),
    vscode.window.registerUriHandler({
      handleUri: (uri) => {
        void handleUri(uri).catch(reportError);
      },
    }),
    vscode.commands.registerCommand("changewalk.openReviewFromFile", async (fileArg?: unknown) => {
      // A string argument lets the CLI, a test, or a keybinding open a
      // review straight away; without one we ask.
      const file =
        typeof fileArg === "string"
          ? fileArg
          : await vscode.window.showInputBox({
              prompt: "Path to the changewalk hand-off JSON",
              ignoreFocusOut: true,
            });
      if (file) {
        await openReview({ mr: file }).catch(reportError);
      }
    }),
  );
}

export function deactivate(): void {
  // Nothing to tear down; the webview and diffs are owned by VS Code.
}

async function handleUri(uri: vscode.Uri): Promise<void> {
  await openReview(parseReviewUri(uri.path, uri.query));
}

async function openReview(launch: ReviewLaunch): Promise<void> {
  const handoff = await readHandoff(launch.mr);
  const repo = launch.repo ?? firstWorkspaceFolder();
  if (!repo) {
    throw new Error("no repository for the review; open the clone, or launch from the CLI");
  }

  await showDescription(handoff);
  await openDiff(handoff, repo, launch.base);
}

async function openDiff(handoff: Handoff, repo: string, base: string | undefined): Promise<void> {
  const git = await activateGitExtension();
  if (!git) {
    throw new Error("VS Code's Git extension is not available; cannot show the diff");
  }

  const baseRef = base ?? fallbackBase(handoff.target_branch);
  const baseSha = await resolveBase(repo, baseRef);
  const entries = await changedFiles(repo, baseRef);
  if (entries.length === 0) {
    void vscode.window.showInformationMessage(`${changeNumber(handoff)} has no changed files to review.`);
    return;
  }
  const added = new Set(entries.filter((entry) => entry.added).map((entry) => entry.path));
  const files = order(
    entries.map((entry) => entry.path),
    handoff,
  );

  const resources = files.map((file) => {
    const fileUri = vscode.Uri.file(path.join(repo, file));
    // An added file has no content at the base, so its left side is empty
    // rather than a git URI that would fail to resolve.
    const original = added.has(file)
      ? vscode.Uri.from({ scheme: EMPTY_SCHEME, path: file })
      : git.toGitUri(fileUri, baseSha);
    return [fileUri, original, fileUri] as [vscode.Uri, vscode.Uri, vscode.Uri];
  });

  const title = `Review ${changeNumber(handoff)} · ${handoff.title}`;
  await vscode.commands.executeCommand("vscode.changes", title, resources);
}

async function showDescription(handoff: Handoff): Promise<void> {
  const panel = vscode.window.createWebviewPanel(
    "changewalk.description",
    `${changeNumber(handoff)} · ${handoff.title}`,
    { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
    { enableScripts: false },
  );
  panel.webview.html = descriptionHtml(handoff);
}

function descriptionHtml(handoff: Handoff): string {
  const author = handoff.author?.name ? ` · ${escapeHtml(handoff.author.name)}` : "";
  const counts =
    handoff.additions !== undefined || handoff.deletions !== undefined
      ? `<p class="counts">+${handoff.additions ?? 0} −${handoff.deletions ?? 0}</p>`
      : "";
  const summary = handoff.review?.summary
    ? `<section class="summary"><h2>Summary</h2><p>${escapeHtml(handoff.review.summary)}</p></section>`
    : "";
  const description = handoff.description
    ? `<pre class="body">${escapeHtml(handoff.description)}</pre>`
    : `<p class="empty">No description.</p>`;
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
<style>
  body { font-family: var(--vscode-font-family); padding: 1rem 1.25rem; line-height: 1.5; }
  h1 { font-size: 1.1rem; margin: 0 0 .25rem; }
  .meta { color: var(--vscode-descriptionForeground); margin: 0 0 1rem; }
  .counts { color: var(--vscode-descriptionForeground); margin: 0 0 1rem; }
  .body { white-space: pre-wrap; word-wrap: break-word; font-family: var(--vscode-font-family); }
  .empty { color: var(--vscode-descriptionForeground); font-style: italic; }
  a { color: var(--vscode-textLink-foreground); }
  .summary { border-left: 2px solid var(--vscode-focusBorder); padding-left: .75rem; margin-bottom: 1rem; }
</style>
</head>
<body>
  <h1>${escapeHtml(handoff.title)}</h1>
  <p class="meta">${escapeHtml(changeNumber(handoff))}${author} · <a href="${escapeAttr(handoff.web_url)}">open on host</a></p>
  ${counts}
  ${summary}
  ${description}
</body>
</html>`;
}

function firstWorkspaceFolder(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function reportError(error: unknown): void {
  const message = error instanceof Error ? error.message : String(error);
  void vscode.window.showErrorMessage(`changewalk: ${message}`);
}

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => {
    switch (c) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      default:
        return "&#39;";
    }
  });
}

function escapeAttr(text: string): string {
  return escapeHtml(text);
}
