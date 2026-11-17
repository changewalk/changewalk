// GitHub review data through `gh`: reading review threads, PR-level comments,
// approvals and your standing, and the writes behind the review. The GitHub
// counterpart to gitlab.ts. Pure parsing is exported and tested; the calls go
// through gh.ts.

import { ghApi, ghGraphql } from "./gh";
import { Approvals, Draft, PostFailure, RelatedIssue, RemoteNote, RemoteThread, ReviewerState, Upload } from "./gitlab";
import { DiffRefs } from "./handoff";
import { FileOnGitLab, lineInHead } from "./placement";

/** owner, name and number read off a pull request's web URL. */
export interface PullRef {
  owner: string;
  name: string;
  number: number;
}

/** `https://github.com/<owner>/<name>/pull/<n>` into its parts. */
export function pullRefFromUrl(webUrl: string, fallbackNumber: number): PullRef | undefined {
  const after = webUrl.split("://")[1]?.split("/");
  if (!after || after.length < 3) {
    return undefined;
  }
  const [, owner, name] = after; // after[0] is the host.
  const pullIndex = after.indexOf("pull");
  const number = pullIndex >= 0 ? Number(after[pullIndex + 1]) : fallbackNumber;
  if (!owner || !name || !Number.isFinite(number)) {
    return undefined;
  }
  return { owner, name, number };
}

// ---- Threads ----------------------------------------------------------------

interface RawThreadComment {
  databaseId?: number;
  body?: string;
  createdAt?: string;
  author?: { login?: string } | null;
}

/** On GitHub the path, line and side belong to the thread, not its comments. */
interface RawReviewThread {
  id: string;
  isResolved?: boolean;
  isOutdated?: boolean;
  path?: string;
  line?: number | null;
  originalLine?: number | null;
  startLine?: number | null;
  originalStartLine?: number | null;
  diffSide?: string;
  comments?: { nodes?: RawThreadComment[] };
}

interface RawIssueComment {
  id: number;
  body?: string;
  created_at?: string;
  user?: { login?: string } | null;
}

/** How a thread's writes are routed, learned while its comments are read. */
export interface ThreadRouting {
  /** The node id GraphQL resolves and unresolves by. */
  nodeId?: string;
  /** The database id a reply threads under, for a line thread. */
  rootCommentId?: number;
  /** A PR-level thread is one issue comment; it cannot be resolved. */
  issueComment?: boolean;
}

/** Line threads from the GraphQL reply, and the routing each needs for replies
 *  and resolution. A null line means the thread went stale; it keeps its
 *  original line so it still draws. */
export function lineThreadsFrom(raw: RawReviewThread[]): { threads: RemoteThread[]; routing: Map<string, ThreadRouting> } {
  const threads: RemoteThread[] = [];
  const routing = new Map<string, ThreadRouting>();
  for (const thread of raw) {
    const comments = thread.comments?.nodes ?? [];
    const first = comments[0];
    if (!first || !thread.path) {
      continue;
    }
    const line = thread.line ?? thread.originalLine ?? undefined;
    if (line === undefined) {
      continue;
    }
    const oldSide = thread.diffSide === "LEFT";
    const start = thread.startLine ?? thread.originalStartLine ?? line;
    const notes: RemoteNote[] = comments.map((comment) => ({
      id: comment.databaseId,
      author: comment.author?.login ?? "",
      body: comment.body ?? "",
      createdAt: comment.createdAt,
    }));
    threads.push({
      id: thread.id,
      path: thread.path,
      line,
      startLine: Math.min(start, line),
      oldSide,
      resolved: Boolean(thread.isResolved),
      resolvable: true,
      createdAt: first.createdAt,
      notes,
    });
    routing.set(thread.id, { nodeId: thread.id, rootCommentId: first.databaseId });
  }
  return { threads, routing };
}

/** PR-level comments as change threads. GitHub cannot resolve them, so they are
 *  drawn without a resolve control. */
export function changeThreadsFrom(raw: RawIssueComment[]): { threads: RemoteThread[]; routing: Map<string, ThreadRouting> } {
  const threads: RemoteThread[] = [];
  const routing = new Map<string, ThreadRouting>();
  for (const comment of raw) {
    const id = `issue:${comment.id}`;
    threads.push({
      id,
      oldSide: false,
      resolved: false,
      resolvable: false,
      createdAt: comment.created_at,
      notes: [{ id: comment.id, author: comment.user?.login ?? "", body: comment.body ?? "", createdAt: comment.created_at }],
    });
    routing.set(id, { issueComment: true });
  }
  return { threads, routing };
}

// ---- Approvals and standing --------------------------------------------------

interface RawReview {
  user?: { login?: string } | null;
  state?: string;
  submitted_at?: string;
  id?: number;
}

/** The latest review state each reviewer left, newest wins. Pending and plain
 *  comment reviews do not change an earlier verdict. */
export function latestReviewStates(reviews: RawReview[]): Map<string, string> {
  const latest = new Map<string, string>();
  for (const review of reviews) {
    const login = review.user?.login;
    const state = review.state;
    if (!login || !state || state === "PENDING") {
      continue;
    }
    // COMMENTED does not overturn a prior APPROVED or CHANGES_REQUESTED.
    if (state === "COMMENTED" && latest.has(login) && latest.get(login) !== "COMMENTED") {
      continue;
    }
    latest.set(login, state);
  }
  return latest;
}

export function approvalsFrom(reviews: RawReview[], me: string): Approvals {
  const latest = latestReviewStates(reviews);
  const approvedBy: string[] = [];
  for (const [login, state] of latest) {
    if (state === "APPROVED") {
      approvedBy.push(login);
    }
  }
  return { approvedBy, userHasApproved: latest.get(me) === "APPROVED" };
}

export function reviewerStateFrom(reviews: RawReview[], me: string): ReviewerState {
  switch (latestReviewStates(reviews).get(me)) {
    case "APPROVED":
      return "approved";
    case "CHANGES_REQUESTED":
      return "requested_changes";
    case "COMMENTED":
      return "reviewed";
    default:
      return "unreviewed";
  }
}

// ---- Placing a draft on the diff --------------------------------------------

/** A draft as a GitHub review comment, or why GitHub would not take it. The
 *  draft's lines count in the file on disk; they are mapped onto head first,
 *  since GitHub comments on the head commit's diff. */
export function placeGithubComment(
  file: FileOnGitLab,
  draft: Draft,
  refs: DiffRefs,
): { comment: Record<string, unknown> } | { error: string } {
  const head = refs.head_sha.slice(0, 7);
  const toHead = (local: number): number => {
    const line = lineInHead(file.sinceHead, local);
    if (line === undefined) {
      throw new PlacementError(
        `${file.path}:${local} is not in the pull request's head commit ${head}; the file on disk differs ` +
          `from what GitHub has, so push the branch and place the comment again`,
      );
    }
    if (!file.diff.lines.has(line)) {
      throw new PlacementError(
        `${file.path}:${local} is outside the pull request's diff on GitHub, which only takes comments on ` +
          `changed lines and the lines around them`,
      );
    }
    return line;
  };
  try {
    const line = toHead(draft.line);
    const startLine = toHead(Math.min(draft.startLine, draft.line));
    const comment: Record<string, unknown> = { path: file.path, line, side: "RIGHT", body: draft.body };
    if (startLine < line) {
      comment.start_line = startLine;
      comment.start_side = "RIGHT";
    }
    return { comment };
  } catch (error) {
    if (error instanceof PlacementError) {
      return { error: error.message };
    }
    throw error;
  }
}

class PlacementError extends Error {}

// ---- The backend -------------------------------------------------------------

const PAGE_SIZE = 100;
const MAX_PAGES = 50;

/** Reviews GitHub through the `gh` CLI, in the clone so `gh` picks the login. */
export class GitHubBackend {
  private readonly routing = new Map<string, ThreadRouting>();
  private loginPromise: Promise<string> | undefined;

  constructor(
    private readonly repo: string,
    private readonly ref: PullRef,
  ) {}

  private repoPath(rest: string): string {
    return `repos/${this.ref.owner}/${this.ref.name}/${rest}`;
  }

  private async login(): Promise<string> {
    if (!this.loginPromise) {
      this.loginPromise = ghApi(this.repo, "GET", "user").then((text) => (JSON.parse(text) as { login: string }).login);
    }
    return this.loginPromise;
  }

  async fetchThreads(): Promise<{ onLines: RemoteThread[]; onChange: RemoteThread[] }> {
    this.routing.clear();
    const [lines, change] = await Promise.all([this.fetchLineThreads(), this.fetchChangeThreads()]);
    for (const [id, route] of lines.routing) {
      this.routing.set(id, route);
    }
    for (const [id, route] of change.routing) {
      this.routing.set(id, route);
    }
    return { onLines: lines.threads, onChange: change.threads };
  }

  private async fetchLineThreads(): Promise<{ threads: RemoteThread[]; routing: Map<string, ThreadRouting> }> {
    const query = `query($owner:String!,$name:String!,$number:Int!,$cursor:String){
      repository(owner:$owner,name:$name){ pullRequest(number:$number){ reviewThreads(first:100,after:$cursor){
        pageInfo{ hasNextPage endCursor }
        nodes{ id isResolved isOutdated path line originalLine startLine originalStartLine diffSide
          comments(first:100){ nodes{ databaseId body createdAt author{ login } } } } } } } }`;
    const all: RawReviewThread[] = [];
    let cursor: string | null = null;
    for (let page = 0; page < MAX_PAGES; page++) {
      const reply = JSON.parse(
        await ghGraphql(this.repo, query, { owner: this.ref.owner, name: this.ref.name, number: this.ref.number, cursor }),
      ) as {
        data?: { repository?: { pullRequest?: { reviewThreads?: { nodes?: RawReviewThread[]; pageInfo?: { hasNextPage?: boolean; endCursor?: string } } } } };
      };
      const field = reply.data?.repository?.pullRequest?.reviewThreads;
      all.push(...(field?.nodes ?? []));
      if (!field?.pageInfo?.hasNextPage || !field.pageInfo.endCursor) {
        break;
      }
      cursor = field.pageInfo.endCursor;
    }
    return lineThreadsFrom(all);
  }

  private async fetchChangeThreads(): Promise<{ threads: RemoteThread[]; routing: Map<string, ThreadRouting> }> {
    const raw: RawIssueComment[] = [];
    for (let page = 1; page <= MAX_PAGES; page++) {
      const batch = JSON.parse(
        await ghApi(this.repo, "GET", this.repoPath(`issues/${this.ref.number}/comments?per_page=${PAGE_SIZE}&page=${page}`)),
      ) as RawIssueComment[];
      raw.push(...batch);
      if (batch.length < PAGE_SIZE) {
        break;
      }
    }
    return changeThreadsFrom(raw);
  }

  async replyToThread(discussionId: string, body: string): Promise<void> {
    const route = this.routing.get(discussionId);
    if (route?.issueComment || discussionId.startsWith("issue:")) {
      await this.postNote(body);
      return;
    }
    if (route?.rootCommentId === undefined) {
      throw new Error("this thread cannot be replied to yet; reload the review");
    }
    await ghApi(this.repo, "POST", this.repoPath(`pulls/${this.ref.number}/comments/${route.rootCommentId}/replies`), { body });
  }

  async setThreadResolved(discussionId: string, resolved: boolean): Promise<void> {
    const route = this.routing.get(discussionId);
    const nodeId = route?.nodeId ?? (discussionId.startsWith("issue:") ? undefined : discussionId);
    if (!nodeId) {
      throw new Error("a pull request comment on the change itself cannot be resolved on GitHub");
    }
    const mutation = resolved
      ? `mutation($id:ID!){ resolveReviewThread(input:{threadId:$id}){ thread{ id } } }`
      : `mutation($id:ID!){ unresolveReviewThread(input:{threadId:$id}){ thread{ id } } }`;
    await ghGraphql(this.repo, mutation, { id: nodeId });
  }

  async updateNote(discussionId: string, noteId: number, body: string): Promise<void> {
    const route = this.routing.get(discussionId);
    const endpoint =
      route?.issueComment || discussionId.startsWith("issue:")
        ? this.repoPath(`issues/comments/${noteId}`)
        : this.repoPath(`pulls/comments/${noteId}`);
    await ghApi(this.repo, "PATCH", endpoint, { body });
  }

  /** On GitHub a thread starts from a plain pull request comment. */
  async startThread(body: string): Promise<void> {
    await this.postNote(body);
  }

  async postNote(body: string): Promise<void> {
    await ghApi(this.repo, "POST", this.repoPath(`issues/${this.ref.number}/comments`), { body });
  }

  async postDrafts(
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
        const placed = placeGithubComment(await files.get(draft.path)!, draft, refs);
        if ("error" in placed) {
          failed.push({ id: draft.id, error: placed.error });
          continue;
        }
        await ghApi(this.repo, "POST", this.repoPath(`pulls/${this.ref.number}/comments`), {
          commit_id: refs.head_sha,
          ...placed.comment,
        });
        posted.push(draft.id);
      } catch (error) {
        failed.push({ id: draft.id, error: `${draft.path}:${draft.line}: ${error instanceof Error ? error.message : String(error)}` });
      }
    }
    return { posted, failed };
  }

  async fetchApprovals(): Promise<Approvals> {
    const [reviews, me] = await Promise.all([this.fetchReviews(), this.login()]);
    return approvalsFrom(reviews, me);
  }

  async fetchReviewerState(): Promise<ReviewerState> {
    const [reviews, me] = await Promise.all([this.fetchReviews(), this.login()]);
    return reviewerStateFrom(reviews, me);
  }

  private async fetchReviews(): Promise<RawReview[]> {
    const reviews: RawReview[] = [];
    for (let page = 1; page <= MAX_PAGES; page++) {
      const batch = JSON.parse(
        await ghApi(this.repo, "GET", this.repoPath(`pulls/${this.ref.number}/reviews?per_page=${PAGE_SIZE}&page=${page}`)),
      ) as RawReview[];
      reviews.push(...batch);
      if (batch.length < PAGE_SIZE) {
        break;
      }
    }
    return reviews;
  }

  /** Approve, or withdraw it by dismissing the last approval of your own. */
  async setApproval(approve: boolean): Promise<void> {
    if (approve) {
      await ghApi(this.repo, "POST", this.repoPath(`pulls/${this.ref.number}/reviews`), { event: "APPROVE" });
      return;
    }
    await this.dismissMyLatest("APPROVED", "Approval withdrawn.");
  }

  async setChangesRequested(requested: boolean, body?: string): Promise<void> {
    if (requested) {
      await ghApi(this.repo, "POST", this.repoPath(`pulls/${this.ref.number}/reviews`), {
        event: "REQUEST_CHANGES",
        body: body && body.trim() ? body.trim() : "Requesting changes.",
      });
      return;
    }
    await this.dismissMyLatest("CHANGES_REQUESTED", "Request for changes withdrawn.");
  }

  private async dismissMyLatest(state: string, message: string): Promise<void> {
    const [reviews, me] = await Promise.all([this.fetchReviews(), this.login()]);
    const mine = reviews.filter((review) => review.user?.login === me && review.state === state && review.id !== undefined);
    const last = mine[mine.length - 1];
    if (!last) {
      return;
    }
    await ghApi(this.repo, "PUT", this.repoPath(`pulls/${this.ref.number}/reviews/${last.id}/dismissals`), {
      message,
      event: "DISMISS",
    });
  }

  async fetchRelatedIssues(): Promise<RelatedIssue[]> {
    const query = `query($owner:String!,$name:String!,$number:Int!){
      repository(owner:$owner,name:$name){ pullRequest(number:$number){
        closingIssuesReferences(first:50){ nodes{ number title url } } } } }`;
    const reply = JSON.parse(await ghGraphql(this.repo, query, { owner: this.ref.owner, name: this.ref.name, number: this.ref.number })) as {
      data?: { repository?: { pullRequest?: { closingIssuesReferences?: { nodes?: { number: number; title: string; url: string }[] } } } };
    };
    return (reply.data?.repository?.pullRequest?.closingIssuesReferences?.nodes ?? []).map((issue) => ({
      iid: issue.number,
      title: issue.title,
      web_url: issue.url,
    }));
  }

  // GitHub draws description pictures from their https links directly, so there
  // is nothing to prefetch and no project upload to resolve.
  imageUploads(_description: string): Upload[] {
    return [];
  }

  uploadFromLink(_link: string): Upload | undefined {
    return undefined;
  }

  uploadCacheName(upload: Upload): string {
    return `${upload.secret}-${upload.fileName}`;
  }

  async fetchUpload(_upload: Upload): Promise<Buffer> {
    throw new Error("GitHub description pictures are drawn from their links; nothing to fetch");
  }
}
