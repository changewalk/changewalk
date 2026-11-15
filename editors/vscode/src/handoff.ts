// The hand-off the CLI writes before it launches the editor: the change in
// GitLab's shape (see the Rust `MergeRequest` in `cli/src/forge.rs`), plus a
// `review` field with the model's reading order and notes when the CLI has
// them. Only the fields the extension reads are declared here.

import { promises as fs } from "fs";

export type Forge = "gitlab" | "github";

export interface Author {
  name: string;
  username?: string;
}

/** A note the model left on one line of a file. */
export interface ReviewNote {
  /** Line in the new (right) side of the file, 1-based. */
  line: number;
  text: string;
}

/** One changed file, in the order the review should read it. */
export interface OrderedFile {
  path: string;
  /** Why the file is placed here, one short sentence. */
  reason?: string;
  notes?: ReviewNote[];
}

export interface DiffRefs {
  base_sha: string;
  head_sha: string;
  start_sha: string;
}

/** The reading order and summary. All optional: the renderer degrades to a
 *  plain diff when a part is absent. */
export interface ReviewPayload {
  order?: OrderedFile[];
  summary?: string;
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
  /** The clone the change belongs to. */
  repo?: string;
  additions?: number;
  deletions?: number;
  comment_count?: number;
  approved_by?: string[];
  user_has_approved?: boolean;
  /** Present on a single change; the commits the diff is between. */
  diff_refs?: DiffRefs | null;
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
