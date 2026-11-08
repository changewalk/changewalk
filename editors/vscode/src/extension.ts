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
import { activateGitExtension, changedFiles, fallbackBase, resolveBase } from "./git";

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.window.registerUriHandler({
      handleUri: (uri) => {
        void handleUri(uri).catch(reportError);
      },
    }),
    vscode.commands.registerCommand("changewalk.openReviewFromFile", async () => {
      const file = await vscode.window.showInputBox({
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

interface ReviewLaunch {
  /** Path to the hand-off JSON. */
  mr: string;
  /** The clone the change belongs to. Falls back to the JSON's own repo. */
  repo?: string;
  /** What to diff against. Falls back to the target branch. */
  base?: string;
}

async function handleUri(uri: vscode.Uri): Promise<void> {
  const action = uri.path.replace(/^\/+/, "");
  if (action !== "review") {
    throw new Error(`unknown changewalk action: ${uri.path || "(none)"}`);
  }
  const query = new URLSearchParams(uri.query);
  const mr = query.get("mr");
  if (!mr) {
    throw new Error("the review URL has no mr= hand-off file");
  }
  await openReview({ mr, repo: query.get("repo") ?? undefined, base: query.get("base") ?? undefined });
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
  const files = order(await changedFiles(repo, baseRef), handoff);
  if (files.length === 0) {
    void vscode.window.showInformationMessage(`${changeNumber(handoff)} has no changed files to review.`);
    return;
  }

  const resources = files.map((file) => {
    const fileUri = vscode.Uri.file(path.join(repo, file));
    const original = git.toGitUri(fileUri, baseSha);
    return [fileUri, original, fileUri] as [vscode.Uri, vscode.Uri, vscode.Uri];
  });

  const title = `Review ${changeNumber(handoff)} · ${handoff.title}`;
  await vscode.commands.executeCommand("vscode.changes", title, resources);
}

/** Puts the files the CLI's reading order names first, in that order, then
 *  any the order did not mention, in git's order. Absent order: git's order. */
function order(files: string[], handoff: Handoff): string[] {
  const reading = handoff.review?.order;
  if (!reading || reading.length === 0) {
    return files;
  }
  const present = new Set(files);
  const ordered = reading.map((entry) => entry.path).filter((file) => present.has(file));
  const seen = new Set(ordered);
  return [...ordered, ...files.filter((file) => !seen.has(file))];
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
