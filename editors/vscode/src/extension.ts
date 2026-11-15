// changewalk thin client for VS Code and Cursor.
//
// The CLI checks the change's branch out, writes it as JSON, and opens
//   vscode://changewalk.changewalk/review?repo=<dir>&base=<ref>&mr=<file>
// (Cursor uses the cursor:// scheme; the handler is the same). We read the
// hand-off, open an ordered multi-file diff of the change, and open the review
// session: threads, notes, drafts, verdicts, and the description panel.

import * as vscode from "vscode";
import * as path from "path";
import { Handoff, OrderedFile, changeNumber, readHandoff } from "./handoff";
import { GitApi, activateGitExtension } from "./git";
import { ReviewLaunch, changedFiles, fallbackBase, order, parseReviewUri, resolveBase } from "./review";
import { ReviewSession } from "./session";

/** A URI scheme whose content is always empty, for the base side of an
 *  added file's diff. Works the same in VS Code and Cursor. */
const EMPTY_SCHEME = "changewalk-empty";

/** The review open now. One at a time: opening another replaces it. */
let active: ReviewSession | undefined;
let extensionContext: vscode.ExtensionContext | undefined;

export function activate(context: vscode.ExtensionContext): void {
  extensionContext = context;
  const run = (fn: (session: ReviewSession) => Promise<void>) => async (): Promise<void> => {
    if (!active) {
      throw new Error("no review is open; open one from the CLI first");
    }
    await fn(active);
  };
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
    vscode.commands.registerCommand("changewalk.postReview", () => run((s) => s.postReview())().catch(reportError)),
    vscode.commands.registerCommand("changewalk.approve", () => run((s) => s.verdict("approve"))().catch(reportError)),
    vscode.commands.registerCommand("changewalk.requestChanges", () =>
      run((s) => s.verdict("requestChanges"))().catch(reportError),
    ),
    vscode.commands.registerCommand("changewalk.markReviewed", () => run((s) => s.markReviewed())().catch(reportError)),
    vscode.commands.registerCommand("changewalk.replyToThread", (reply: vscode.CommentReply) =>
      run((s) => s.reply(reply))().catch(reportError),
    ),
    vscode.commands.registerCommand("changewalk.toggleResolved", (thread: vscode.CommentThread) =>
      run((s) => s.toggleResolved(thread))().catch(reportError),
    ),
    vscode.commands.registerCommand("changewalk.editComment", (comment: vscode.Comment) =>
      run((s) => s.editComment(comment))().catch(reportError),
    ),
    vscode.commands.registerCommand("changewalk.discardDraft", (thread: vscode.CommentThread) =>
      run(async (s) => s.discardDraft(thread))().catch(reportError),
    ),
    { dispose: () => active?.dispose() },
  );
}

export function deactivate(): void {
  active?.dispose();
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
  const git = await activateGitExtension();
  if (!git) {
    throw new Error("VS Code's Git extension is not available; cannot show the diff");
  }

  const baseRef = launch.base ?? fallbackBase(handoff.target_branch);
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

  await openDiff(handoff, repo, git, baseSha, files, added);

  active?.dispose();
  void vscode.commands.executeCommand("setContext", "changewalk.reviewing", true);
  active = new ReviewSession({
    context: requireContext(),
    handoff,
    repo,
    reading: readingOrder(handoff, files),
    files,
    leftUri: (file) => (added.has(file) ? undefined : git.toGitUri(vscode.Uri.file(path.join(repo, file)), baseSha)),
    openChangesSinceReview: (sha) => openDiffAgainst(handoff, repo, git, sha),
  });
  await active.start();
}

/** Opens the changed files as one multi-file diff against `baseSha`. */
async function openDiff(
  handoff: Handoff,
  repo: string,
  git: GitApi,
  baseSha: string,
  files: string[],
  added: Set<string>,
): Promise<void> {
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

/** Opens the diff against a commit, with the same files in the same order. */
async function openDiffAgainst(handoff: Handoff, repo: string, git: GitApi, sha: string): Promise<void> {
  const entries = await changedFiles(repo, sha);
  if (entries.length === 0) {
    void vscode.window.showInformationMessage("Nothing has changed since your review.");
    return;
  }
  const added = new Set(entries.filter((entry) => entry.added).map((entry) => entry.path));
  await openDiff(handoff, repo, git, sha, entries.map((entry) => entry.path), added);
}

/** The reading order of the files, with each file's reason and notes where the CLI sent them. */
function readingOrder(handoff: Handoff, files: string[]): OrderedFile[] {
  const byPath = new Map((handoff.review?.order ?? []).map((entry) => [entry.path, entry]));
  return files.map((file) => byPath.get(file) ?? { path: file });
}

function requireContext(): vscode.ExtensionContext {
  if (!extensionContext) {
    throw new Error("changewalk is not activated");
  }
  return extensionContext;
}

function firstWorkspaceFolder(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function reportError(error: unknown): void {
  const message = error instanceof Error ? error.message : String(error);
  void vscode.window.showErrorMessage(`changewalk: ${message}`);
}
