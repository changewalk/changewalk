// The hand-off the CLI writes before it launches the editor.
//
// Today the CLI writes a single change in GitLab's shape (see the Rust
// `MergeRequest` in `cli/src/forge.rs`) to a temp file, and passes that
// file's path as the `mr` query of the launch URL. This module reads only
// the fields the thin client needs.
//
// The `review` field is not written yet. It is the shared payload the CLI
// review engine will add in a later phase (threads, reading order, notes,
// summary). The types are declared now so the renderer can light up each
// part as the CLI starts to send it, without a format change here.

import { promises as fs } from "fs";

export type Forge = "gitlab" | "github";

export interface Author {
  name: string;
  username?: string;
}

/** One changed file, in the order the review should read it. */
export interface OrderedFile {
  path: string;
  /** Why this file is placed here, shown as a guiding note. Optional. */
  note?: string;
}

/** One review thread, anchored to a file and line in the diff. */
export interface ReviewThread {
  id: string;
  path: string;
  /** Line in the new (right) side of the diff, 1-based. */
  line: number;
  resolved: boolean;
  comments: ReviewComment[];
}

export interface ReviewComment {
  author: string;
  body: string;
  /** RFC 3339. */
  at: string;
}

/** The shared review payload the CLI engine will add. All optional, so the
 *  renderer degrades to a plain ordered diff when a part is absent. */
export interface ReviewPayload {
  order?: OrderedFile[];
  summary?: string;
  threads?: ReviewThread[];
}

export interface Handoff {
  iid: number;
  title: string;
  description?: string | null;
  author?: Author | null;
  source_branch: string;
  target_branch: string;
  web_url: string;
  forge: Forge;
  additions?: number;
  deletions?: number;
  comment_count?: number;
  review?: ReviewPayload;
}

/** `!1628` on GitLab, `#1628` on GitHub, as each host writes a number. */
export function changeNumber(handoff: Handoff): string {
  const prefix = handoff.forge === "github" ? "#" : "!";
  return `${prefix}${handoff.iid}`;
}

export async function readHandoff(file: string): Promise<Handoff> {
  const text = await fs.readFile(file, "utf8");
  return JSON.parse(text) as Handoff;
}
