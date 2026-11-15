// Places a local comment on the merge request's diff, as GitLab needs it.
//
// GitLab takes a comment only on a line of its own diff (base_sha..head_sha).
// The file on disk can be ahead of, behind, or edited since head_sha, so each
// line is first mapped onto head_sha, then looked up in the diff to learn
// whether it is added or unchanged. Pure: the git reads are in review.ts.

import { createHash } from "crypto";
import { DiffRefs } from "./handoff";

export interface Hunk {
  oldStart: number;
  oldLen: number;
  newStart: number;
  newLen: number;
}

/** A line of the merge request's diff, keyed by its number in the new file. */
export interface DiffLine {
  /** The line's number in the old file when unchanged; absent when added. */
  oldLine?: number;
  /** The old side's counter when the diff reached this line. */
  oldPosition: number;
}

/** The lines of one file that GitLab's diff of the merge request shows. */
export interface DiffFile {
  oldPath?: string;
  lines: Map<number, DiffLine>;
}

/** One file as GitLab holds it, next to how the file on disk differs from it. */
export interface FileOnGitLab {
  path: string;
  diff: DiffFile;
  /** Changes from head_sha to the file on disk, with no context lines. */
  sinceHead: Hunk[];
}

export interface Comment {
  path: string;
  startLine: number;
  line: number;
  body: string;
}

/** Reads `@@ -a,b +c,d @@`; a range without a count covers one line. */
export function parseHunkHeader(line: string): Hunk | undefined {
  const match = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(line);
  if (!match) {
    return undefined;
  }
  return {
    oldStart: Number(match[1]),
    oldLen: match[2] === undefined ? 1 : Number(match[2]),
    newStart: Number(match[3]),
    newLen: match[4] === undefined ? 1 : Number(match[4]),
  };
}

/** The lines a unified diff of one file shows, as `git diff -U3` prints it. */
export function parseDiffFile(text: string): DiffFile {
  const file: DiffFile = { lines: new Map() };
  let inHunk = false;
  let old = 0;
  let next = 0;
  for (const line of text.split("\n")) {
    const hunk = parseHunkHeader(line);
    if (hunk) {
      inHunk = true;
      old = hunk.oldStart;
      next = hunk.newStart;
      continue;
    }
    if (!inHunk) {
      if (line.startsWith("--- a/")) {
        file.oldPath = line.slice("--- a/".length);
      }
      continue;
    }
    switch (line[0]) {
      case "+":
        file.lines.set(next, { oldPosition: old });
        next += 1;
        break;
      case "-":
        old += 1;
        break;
      case " ":
        file.lines.set(next, { oldLine: old, oldPosition: old });
        old += 1;
        next += 1;
        break;
      default:
        // "\ No newline at end of file", or the header of another file.
        break;
    }
  }
  return file;
}

/** The hunks of `git diff -U0 head -- path`. */
export function parseHunks(text: string): Hunk[] {
  return text.split("\n").flatMap((line) => parseHunkHeader(line) ?? []);
}

/** Where line `local` of the file on disk sits in head_sha, if it is unchanged
 *  between the two. Undefined when the line itself was changed. */
export function lineInHead(hunks: Hunk[], local: number): number | undefined {
  let shift = 0;
  for (const hunk of hunks) {
    if (hunk.newLen > 0 && local >= hunk.newStart && local < hunk.newStart + hunk.newLen) {
      return undefined;
    }
    const hunkEnd = hunk.newLen === 0 ? hunk.newStart : hunk.newStart + hunk.newLen - 1;
    if (local > hunkEnd) {
      shift += hunk.oldLen - hunk.newLen;
    }
  }
  const head = local + shift;
  return head >= 1 ? head : undefined;
}

/** GitLab's line code: SHA-1 of the path, then the old and new line numbers. */
function lineCode(path: string, oldPosition: number, newLine: number): string {
  return `${createHash("sha1").update(path).digest("hex")}_${oldPosition}_${newLine}`;
}

/** The discussion body GitLab takes for `comment`, or why it would not take it. */
export function placeComment(
  file: FileOnGitLab,
  comment: Comment,
  refs: DiffRefs,
): { position: Record<string, unknown> } | { error: string } {
  const head = refs.head_sha.slice(0, 7);
  const inHead = (local: number): number => {
    const line = lineInHead(file.sinceHead, local);
    if (line === undefined) {
      throw new PlacementError(
        `${file.path}:${local} is not in the merge request's head commit ${head}; the file on disk ` +
          `differs from what GitLab has, so update or push the branch and place the comment again`,
      );
    }
    return line;
  };
  const onDiff = (newLine: number): DiffLine => {
    const line = file.diff.lines.get(newLine);
    if (!line) {
      throw new PlacementError(
        `${file.path}:${newLine} is outside the merge request's diff on GitLab, which only takes ` +
          `comments on changed lines and the three lines around them`,
      );
    }
    return line;
  };
  try {
    const line = inHead(comment.line);
    const startLine = inHead(Math.min(comment.startLine, comment.line));
    const end = onDiff(line);
    const start = onDiff(startLine);

    const position: Record<string, unknown> = {
      position_type: "text",
      base_sha: refs.base_sha,
      head_sha: refs.head_sha,
      start_sha: refs.start_sha,
      new_path: file.path,
      old_path: file.diff.oldPath ?? file.path,
      new_line: line,
    };
    if (end.oldLine !== undefined) {
      position.old_line = end.oldLine;
    }
    if (startLine < line) {
      const rangeEnd = (newLine: number, diffLine: DiffLine) => ({
        line_code: lineCode(file.path, diffLine.oldPosition, newLine),
        type: diffLine.oldLine === undefined ? "new" : null,
        old_line: diffLine.oldLine ?? null,
        new_line: newLine,
      });
      position.line_range = { start: rangeEnd(startLine, start), end: rangeEnd(line, end) };
    }
    return { position };
  } catch (error) {
    if (error instanceof PlacementError) {
      return { error: error.message };
    }
    throw error;
  }
}

class PlacementError extends Error {}
