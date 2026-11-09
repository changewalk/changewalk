// The review logic that does not touch the VS Code API, kept apart so it
// can be tested headlessly. The glue that opens diffs and webviews lives in
// extension.ts; the Git-extension wrapper in git.ts.

import { execFile } from "child_process";
import { promisify } from "util";
import { Handoff } from "./handoff";

const run = promisify(execFile);

export interface ReviewLaunch {
  /** Path to the hand-off JSON. */
  mr: string;
  /** The clone the change belongs to. */
  repo?: string;
  /** What to diff against. Falls back to the target branch. */
  base?: string;
}

/** A file the change touched. `added` files have no content at the base, so
 *  the diff shows them against an empty left side rather than erroring. */
export interface ChangedFile {
  path: string;
  added: boolean;
}

/** Reads a `scheme://.../review?repo=&base=&mr=` URL into a launch. Throws
 *  with a clear message when the action is not `review` or `mr` is missing. */
export function parseReviewUri(path: string, query: string): ReviewLaunch {
  const action = path.replace(/^\/+/, "");
  if (action !== "review") {
    throw new Error(`unknown changewalk action: ${path || "(none)"}`);
  }
  const params = new URLSearchParams(query);
  const mr = params.get("mr");
  if (!mr) {
    throw new Error("the review URL has no mr= hand-off file");
  }
  return { mr, repo: params.get("repo") ?? undefined, base: params.get("base") ?? undefined };
}

/** What to diff the change against when the launch did not say. */
export function fallbackBase(targetBranch: string): string {
  return `origin/${targetBranch}`;
}

/** Puts the files the CLI's reading order names first, in that order, then
 *  any the order did not mention, in git's order. Absent order: git's order. */
export function order(files: string[], handoff: Handoff): string[] {
  const reading = handoff.review?.order;
  if (!reading || reading.length === 0) {
    return files;
  }
  const present = new Set(files);
  const ordered = reading.map((entry) => entry.path).filter((file) => present.has(file));
  const seen = new Set(ordered);
  return [...ordered, ...files.filter((file) => !seen.has(file))];
}

/** The files changed between `base` and HEAD, in git's order, each marked
 *  added or not. Three dots: from the merge base, so commits the base
 *  already has are left out, as the host's own diff does. Deleted files are
 *  left out (nothing in the working tree to show); renames are split into an
 *  add so each side has a real path. */
export async function changedFiles(repo: string, base: string): Promise<ChangedFile[]> {
  const { stdout } = await run(
    "git",
    ["diff", "--name-status", "--no-renames", "--diff-filter=d", `${base}...HEAD`],
    { cwd: repo, maxBuffer: 16 * 1024 * 1024 },
  );
  const files: ChangedFile[] = [];
  for (const line of stdout.split("\n")) {
    const [status, path] = line.split("\t");
    if (status && path) {
      files.push({ path: path.trim(), added: status.trim().startsWith("A") });
    }
  }
  return files;
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
