// The review backend seam. One open review talks to its host through this
// interface; the session does not know whether it is GitLab (through glab) or
// GitHub (through gh). Each backend is bound to one change at construction.

import * as gl from "./gitlab";
import { Approvals, Draft, PostFailure, RelatedIssue, RemoteThread, ReviewerState, Upload } from "./gitlab";
import { GitHubBackend, pullRefFromUrl } from "./github";
import { DiffRefs, Handoff } from "./handoff";
import { FileOnGitLab } from "./placement";

export interface ReviewBackend {
  /** Threads on lines, and threads on the change itself. */
  fetchThreads(): Promise<{ onLines: RemoteThread[]; onChange: RemoteThread[] }>;
  fetchApprovals(): Promise<Approvals>;
  fetchReviewerState(): Promise<ReviewerState>;
  fetchRelatedIssues(): Promise<RelatedIssue[]>;
  replyToThread(discussionId: string, body: string): Promise<void>;
  setThreadResolved(discussionId: string, resolved: boolean): Promise<void>;
  updateNote(discussionId: string, noteId: number, body: string): Promise<void>;
  startThread(body: string): Promise<void>;
  /** A plain comment on the change, how the review's summary is posted. */
  postNote(body: string): Promise<void>;
  postDrafts(
    refs: DiffRefs,
    drafts: Draft[],
    fileFor: (path: string) => Promise<FileOnGitLab>,
  ): Promise<{ posted: string[]; failed: PostFailure[] }>;
  setApproval(approve: boolean): Promise<void>;
  setChangesRequested(requested: boolean, body?: string): Promise<void>;
  /** Pictures in the description to prefetch; empty when they are plain links. */
  imageUploads(description: string): Upload[];
  uploadCacheName(upload: Upload): string;
  fetchUpload(upload: Upload): Promise<Buffer>;
  /** The upload a description link points at, if the host holds it behind login. */
  uploadFromLink(link: string): Upload | undefined;
}

/** GitLab, through glab, as the review has always worked. */
class GitLabBackend implements ReviewBackend {
  constructor(
    private readonly repo: string,
    private readonly handoff: Handoff,
  ) {}

  private get iid(): number {
    return this.handoff.iid;
  }

  fetchThreads(): Promise<{ onLines: RemoteThread[]; onChange: RemoteThread[] }> {
    return gl.fetchThreads(this.repo, this.iid);
  }

  fetchApprovals(): Promise<Approvals> {
    return gl.fetchApprovals(this.repo, this.iid);
  }

  fetchReviewerState(): Promise<ReviewerState> {
    return gl.fetchReviewerState(this.repo, this.iid);
  }

  fetchRelatedIssues(): Promise<RelatedIssue[]> {
    return gl.fetchRelatedIssues(this.repo, this.iid);
  }

  replyToThread(discussionId: string, body: string): Promise<void> {
    return gl.replyToThread(this.repo, this.iid, discussionId, body);
  }

  setThreadResolved(discussionId: string, resolved: boolean): Promise<void> {
    return gl.setThreadResolved(this.repo, this.iid, discussionId, resolved);
  }

  updateNote(discussionId: string, noteId: number, body: string): Promise<void> {
    return gl.updateNote(this.repo, this.iid, discussionId, noteId, body);
  }

  startThread(body: string): Promise<void> {
    return gl.startThread(this.repo, this.iid, body);
  }

  postNote(body: string): Promise<void> {
    return gl.postNote(this.repo, this.iid, body);
  }

  postDrafts(
    refs: DiffRefs,
    drafts: Draft[],
    fileFor: (path: string) => Promise<FileOnGitLab>,
  ): Promise<{ posted: string[]; failed: PostFailure[] }> {
    return gl.postDrafts(this.repo, this.iid, refs, drafts, fileFor);
  }

  setApproval(approve: boolean): Promise<void> {
    return gl.setApproval(this.repo, this.iid, approve);
  }

  setChangesRequested(requested: boolean): Promise<void> {
    const projectPath = gl.projectPathFromUrl(this.handoff.web_url);
    if (!projectPath) {
      throw new Error("the merge request's project path is unknown");
    }
    return gl.setChangesRequested(this.repo, projectPath, this.iid, requested);
  }

  imageUploads(description: string): Upload[] {
    return gl.uploadsIn(description).filter(gl.isImage);
  }

  uploadCacheName(upload: Upload): string {
    return gl.uploadCacheName(upload);
  }

  fetchUpload(upload: Upload): Promise<Buffer> {
    return gl.fetchUpload(this.repo, upload);
  }

  uploadFromLink(link: string): Upload | undefined {
    return gl.uploadFromLink(link, this.handoff.web_url.split("/-/")[0]);
  }
}

/** The backend for the change, chosen by its forge. */
export function makeBackend(repo: string, handoff: Handoff): ReviewBackend {
  if (handoff.forge === "github") {
    const ref = pullRefFromUrl(handoff.web_url, handoff.iid);
    if (!ref) {
      throw new Error(`could not read the GitHub owner/repo from ${handoff.web_url}`);
    }
    return new GitHubBackend(repo, ref);
  }
  return new GitLabBackend(repo, handoff);
}
