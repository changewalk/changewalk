// GitLab review data: reading threads, approvals and uploads, and the writes
// behind the review. Parsing is pure and tested; the calls go through glab.

import { glabApi, glabApiBytes, glabGraphql } from "./glab";
import { Comment, placeComment, FileOnGitLab } from "./placement";
import { DiffRefs } from "./handoff";

// ---- Threads ---------------------------------------------------------------

export interface RemoteNote {
  id?: number;
  author: string;
  body: string;
  createdAt?: string;
}

/** A thread GitLab holds. On a line when `line` is set; otherwise it sits on
 *  the merge request as a whole. */
export interface RemoteThread {
  id: string;
  path?: string;
  startLine?: number;
  line?: number;
  /** Lines the change removed: the numbers count in the old file. */
  oldSide: boolean;
  resolved: boolean;
  resolvable: boolean;
  createdAt?: string;
  notes: RemoteNote[];
}

interface RawNote {
  id?: number;
  body?: string;
  system?: boolean;
  resolved?: boolean;
  resolvable?: boolean;
  author?: { name?: string } | null;
  created_at?: string;
  position?: RawPosition;
}

interface RawPosition {
  new_path?: string;
  old_path?: string;
  new_line?: number;
  old_line?: number;
  line_range?: { start?: { new_line?: number; old_line?: number } };
}

export interface RawDiscussion {
  id: string;
  notes?: RawNote[];
}

/** Threads from one page of GitLab's discussion list. System notes are left out. */
export function threadsFrom(raw: RawDiscussion[]): { onLines: RemoteThread[]; onChange: RemoteThread[] } {
  const onLines: RemoteThread[] = [];
  const onChange: RemoteThread[] = [];
  for (const discussion of raw) {
    const notes = (discussion.notes ?? []).filter((note) => !note.system);
    const first = notes[0];
    if (!first) {
      continue;
    }
    const remote: RemoteNote[] = notes.map((note) => ({
      id: note.id,
      author: note.author?.name ?? "",
      body: note.body ?? "",
      createdAt: note.created_at,
    }));
    const position = first.position;
    if (!position) {
      // A thread on the merge request itself. Plain comments cannot be resolved, so they stay out.
      if (first.resolvable) {
        onChange.push({
          id: discussion.id,
          oldSide: false,
          resolved: Boolean(first.resolved),
          resolvable: true,
          createdAt: first.created_at,
          notes: remote,
        });
      }
      continue;
    }
    // A thread on a line the change kept or added has a new line; one on a removed line only an old one.
    const rangeStart = position.line_range?.start;
    if (position.new_line !== undefined && position.new_path) {
      onLines.push({
        id: discussion.id,
        path: position.new_path,
        line: position.new_line,
        startLine: Math.min(rangeStart?.new_line ?? position.new_line, position.new_line),
        oldSide: false,
        resolved: Boolean(first.resolved),
        resolvable: Boolean(first.resolvable),
        createdAt: first.created_at,
        notes: remote,
      });
    } else if (position.old_line !== undefined) {
      const path = position.old_path ?? position.new_path;
      if (!path) {
        continue;
      }
      onLines.push({
        id: discussion.id,
        path,
        line: position.old_line,
        startLine: Math.min(rangeStart?.old_line ?? position.old_line, position.old_line),
        oldSide: true,
        resolved: Boolean(first.resolved),
        resolvable: Boolean(first.resolvable),
        createdAt: first.created_at,
        notes: remote,
      });
    }
  }
  return { onLines, onChange };
}

const PAGE_SIZE = 100;
const MAX_PAGES = 50;

/** Every thread of merge request `iid`, reading each page; the newest are on the last. */
export async function fetchThreads(cwd: string, iid: number): Promise<{ onLines: RemoteThread[]; onChange: RemoteThread[] }> {
  const onLines: RemoteThread[] = [];
  const onChange: RemoteThread[] = [];
  for (let page = 1; page <= MAX_PAGES; page++) {
    const raw = JSON.parse(
      await glabApi(cwd, "GET", `projects/:id/merge_requests/${iid}/discussions?per_page=${PAGE_SIZE}&page=${page}`),
    ) as RawDiscussion[];
    const parsed = threadsFrom(raw);
    onLines.push(...parsed.onLines);
    onChange.push(...parsed.onChange);
    if (raw.length < PAGE_SIZE) {
      break;
    }
  }
  return { onLines, onChange };
}

export async function replyToThread(cwd: string, iid: number, discussionId: string, body: string): Promise<void> {
  await glabApi(cwd, "POST", `projects/:id/merge_requests/${iid}/discussions/${discussionId}/notes`, { body });
}

export async function setThreadResolved(cwd: string, iid: number, discussionId: string, resolved: boolean): Promise<void> {
  await glabApi(cwd, "PUT", `projects/:id/merge_requests/${iid}/discussions/${discussionId}`, { resolved });
}

export async function updateNote(cwd: string, iid: number, discussionId: string, noteId: number, body: string): Promise<void> {
  await glabApi(cwd, "PUT", `projects/:id/merge_requests/${iid}/discussions/${discussionId}/notes/${noteId}`, { body });
}

/** Starts a thread on the merge request itself. */
export async function startThread(cwd: string, iid: number, body: string): Promise<void> {
  await glabApi(cwd, "POST", `projects/:id/merge_requests/${iid}/discussions`, { body });
}

/** A general comment on the merge request, which is how the summary is posted. */
export async function postNote(cwd: string, iid: number, body: string): Promise<void> {
  await glabApi(cwd, "POST", `projects/:id/merge_requests/${iid}/notes`, { body });
}

// ---- Posting a review's drafts ----------------------------------------------

export interface Draft extends Comment {
  /** The editor's id for the draft, so it can be dropped once posted. */
  id: string;
}

export interface PostFailure {
  id: string;
  error: string;
}

/** Posts drafts as inline discussions. Each draft is placed on head_sha first;
 *  one that cannot be placed, or that GitLab rejects, is reported and the rest still post. */
export async function postDrafts(
  cwd: string,
  iid: number,
  refs: DiffRefs,
  drafts: Draft[],
  fileFor: (path: string) => Promise<FileOnGitLab>,
): Promise<{ posted: string[]; failed: PostFailure[] }> {
  const posted: string[] = [];
  const failed: PostFailure[] = [];
  const files = new Map<string, Promise<FileOnGitLab>>();
  for (const draft of drafts) {
    try {
      if (!files.has(draft.path)) {
        files.set(draft.path, fileFor(draft.path));
      }
      const placed = placeComment(await files.get(draft.path)!, draft, refs);
      if ("error" in placed) {
        failed.push({ id: draft.id, error: placed.error });
        continue;
      }
      await glabApi(cwd, "POST", `projects/:id/merge_requests/${iid}/discussions`, {
        body: draft.body,
        position: placed.position,
      });
      posted.push(draft.id);
    } catch (error) {
      failed.push({ id: draft.id, error: `${draft.path}:${draft.line}: ${errorText(error)}` });
    }
  }
  return { posted, failed };
}

// ---- Verdicts and standing ---------------------------------------------------

export interface Approvals {
  approvedBy: string[];
  userHasApproved: boolean;
}

export function parseApprovals(text: string): Approvals {
  const raw = JSON.parse(text) as { approved_by?: { user?: { name?: string } }[]; user_has_approved?: boolean };
  return {
    approvedBy: (raw.approved_by ?? []).flatMap((approver) => (approver.user?.name ? [approver.user.name] : [])),
    userHasApproved: Boolean(raw.user_has_approved),
  };
}

export type ReviewerState = "unreviewed" | "reviewed" | "requested_changes" | "approved";

export function reviewerStateFrom(text: string, username: string): ReviewerState {
  const reviewers = JSON.parse(text) as { user?: { username?: string }; state?: string }[];
  const mine = reviewers.find((reviewer) => reviewer.user?.username === username);
  switch (mine?.state) {
    case "reviewed":
      return "reviewed";
    case "requested_changes":
      return "requested_changes";
    case "approved":
      return "approved";
    default:
      return "unreviewed";
  }
}

export async function fetchApprovals(cwd: string, iid: number): Promise<Approvals> {
  return parseApprovals(await glabApi(cwd, "GET", `projects/:id/merge_requests/${iid}/approvals`));
}

export async function fetchReviewerState(cwd: string, iid: number): Promise<ReviewerState> {
  const user = JSON.parse(await glabApi(cwd, "GET", "user")) as { username: string };
  return reviewerStateFrom(await glabApi(cwd, "GET", `projects/:id/merge_requests/${iid}/reviewers`), user.username);
}

export async function setApproval(cwd: string, iid: number, approve: boolean): Promise<void> {
  // An empty object keeps glab from sending a body with no content type.
  await glabApi(cwd, "POST", `projects/:id/merge_requests/${iid}/${approve ? "approve" : "unapprove"}`, {});
}

/** Asks the author for changes, or takes that back. The REST API has no call for it, so GraphQL. */
export async function setChangesRequested(cwd: string, projectPath: string, iid: number, requested: boolean): Promise<void> {
  const mutation = requested ? "mergeRequestRequestChanges" : "mergeRequestDestroyRequestedChanges";
  const query = `mutation { ${mutation}(input: { projectPath: ${JSON.stringify(projectPath)}, iid: ${JSON.stringify(String(iid))} }) { errors } }`;
  const reply = JSON.parse(await glabGraphql(cwd, query)) as {
    data?: Record<string, { errors?: string[] } | undefined>;
    errors?: { message?: string }[];
  };
  const errors = [
    ...(reply.data?.[mutation]?.errors ?? []),
    ...(reply.errors ?? []).flatMap((error) => (error.message ? [error.message] : [])),
  ];
  if (errors.length > 0) {
    throw new Error(errors.join("; "));
  }
}

export interface RelatedIssue {
  iid: number;
  title: string;
  web_url: string;
}

export async function fetchRelatedIssues(cwd: string, iid: number): Promise<RelatedIssue[]> {
  return JSON.parse(await glabApi(cwd, "GET", `projects/:id/merge_requests/${iid}/related_issues`)) as RelatedIssue[];
}

// ---- Uploads in the description ---------------------------------------------

/** A file uploaded to the project. GitLab links it as /uploads/<secret>/<name>, which needs the user's login to fetch. */
export interface Upload {
  secret: string;
  fileName: string;
}

const UPLOADS = "/uploads/";

/** Reads `<secret>/<name>` off the front of `text`, and how much of it that was. */
function parseUpload(text: string): { upload: Upload; length: number } | undefined {
  const slash = text.indexOf("/");
  if (slash < 0) {
    return undefined;
  }
  const secret = text.slice(0, slash);
  if (secret.length < 16 || !/^[0-9a-fA-F]+$/.test(secret)) {
    return undefined;
  }
  const after = text.slice(slash + 1);
  const end = after.search(/[\s)"'<>]/);
  const fileName = end < 0 ? after : after.slice(0, end);
  if (!fileName || fileName.includes("/") || fileName === "." || fileName === "..") {
    return undefined;
  }
  return { upload: { secret, fileName }, length: secret.length + 1 + fileName.length };
}

/** The upload a link points at, if it is one of this project's. */
export function uploadFromLink(link: string, projectUrl?: string): Upload | undefined {
  let path = link;
  if (projectUrl) {
    const base = projectUrl.replace(/\/+$/, "");
    if (link.startsWith(base)) {
      path = link.slice(base.length);
    }
  }
  if (!path.startsWith(UPLOADS)) {
    return undefined;
  }
  const parsed = parseUpload(path.slice(UPLOADS.length));
  return parsed && UPLOADS.length + parsed.length === path.length ? parsed.upload : undefined;
}

/** Every upload linked in `text`, each once, in order. */
export function uploadsIn(text: string): Upload[] {
  const uploads: Upload[] = [];
  let rest = text;
  let start = rest.indexOf(UPLOADS);
  while (start >= 0) {
    rest = rest.slice(start + UPLOADS.length);
    const parsed = parseUpload(rest);
    if (parsed) {
      rest = rest.slice(parsed.length);
      if (!uploads.some((u) => u.secret === parsed.upload.secret && u.fileName === parsed.upload.fileName)) {
        uploads.push(parsed.upload);
      }
    }
    start = rest.indexOf(UPLOADS);
  }
  return uploads;
}

/** Pictures can be drawn; a video keeps its link text. */
export function isImage(upload: Upload): boolean {
  const dot = upload.fileName.lastIndexOf(".");
  const extension = dot < 0 ? "" : upload.fileName.slice(dot + 1).toLowerCase();
  return ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"].includes(extension);
}

/** A name for the fetched file that is safe on every file system and keeps the extension. */
export function uploadCacheName(upload: Upload): string {
  return `${upload.secret}-${upload.fileName.replace(/[^A-Za-z0-9.\-_]/g, "_")}`;
}

export async function fetchUpload(cwd: string, upload: Upload): Promise<Buffer> {
  return glabApiBytes(cwd, "GET", `projects/:id/uploads/${upload.secret}/${upload.fileName}`);
}

// ---- Shared -----------------------------------------------------------------

/** `group/project` off a merge request's web URL, since the API replies carry only a numeric id. */
export function projectPathFromUrl(webUrl: string): string | undefined {
  const afterHost = webUrl.split("://")[1]?.split("/").slice(1).join("/");
  const project = afterHost?.split("/-/")[0];
  return project || undefined;
}

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
