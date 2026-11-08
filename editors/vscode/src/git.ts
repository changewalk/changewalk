// Thin wrappers over git and VS Code's built-in Git extension.
//
// The CLI has already fetched and checked the change's branch out before it
// launches us, so HEAD is the change. We only read: list the changed files
// and ask the built-in Git extension for a file's content at the base, so
// the diff view can show base vs working tree.

import * as vscode from "vscode";
import { execFile } from "child_process";
import { promisify } from "util";

const run = promisify(execFile);

/** The minimal slice of VS Code's Git extension API we use. */
interface GitApi {
  toGitUri(uri: vscode.Uri, ref: string): vscode.Uri;
}

/** The built-in Git extension, present in both VS Code and Cursor. */
export function gitApi(): GitApi | undefined {
  const extension = vscode.extensions.getExtension<{ getAPI(version: 1): GitApi }>("vscode.git");
  return extension?.isActive ? extension.exports.getAPI(1) : undefined;
}

export async function activateGitExtension(): Promise<GitApi | undefined> {
  const extension = vscode.extensions.getExtension<{ getAPI(version: 1): GitApi }>("vscode.git");
  if (!extension) {
    return undefined;
  }
  if (!extension.isActive) {
    await extension.activate();
  }
  return extension.exports.getAPI(1);
}

/** What to diff the change against, when the launch did not say. A two-dot
 *  range from the target's merge base matches what the host shows. */
export function fallbackBase(targetBranch: string): string {
  return `origin/${targetBranch}`;
}

/** The files changed between `base` and HEAD, in git's order. The CLI's
 *  reading order, once sent, reorders these. */
export async function changedFiles(repo: string, base: string): Promise<string[]> {
  // Three dots: from the merge base of `base` and HEAD, so commits the base
  // already has are left out, as the host's own diff does.
  const { stdout } = await run("git", ["diff", "--name-only", "--diff-filter=d", `${base}...HEAD`], {
    cwd: repo,
    maxBuffer: 16 * 1024 * 1024,
  });
  return stdout.split("\n").map((line) => line.trim()).filter((line) => line.length > 0);
}

/** The commit `base` resolves to, so the diff pins a sha rather than a ref
 *  that may move while the review is open. */
export async function resolveBase(repo: string, base: string): Promise<string> {
  const mergeBase = await run("git", ["merge-base", base, "HEAD"], { cwd: repo }).catch(() => undefined);
  if (mergeBase && mergeBase.stdout.trim()) {
    return mergeBase.stdout.trim();
  }
  const { stdout } = await run("git", ["rev-parse", base], { cwd: repo });
  return stdout.trim();
}
