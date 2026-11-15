// One open review in the editor: the comment threads on the diff, the drafts
// a reviewer is writing, the verdict, and the description panel. Everything
// is read from and written to GitLab through glab, as the Zed fork does.

import * as vscode from "vscode";
import { promises as fs } from "fs";
import * as path from "path";
import { randomBytes } from "crypto";
import { Handoff, OrderedFile } from "./handoff";
import * as gl from "./gitlab";
import { Draft, RemoteNote, RemoteThread, errorText } from "./gitlab";
import { DescriptionModel, descriptionHtml } from "./description";
import { loadFileOnGitLab } from "./review";

/** The commit the reviewer last marked as reviewed, per merge request. */
export interface LastReview {
  headSha: string;
  at: string;
}

export interface SessionOptions {
  context: vscode.ExtensionContext;
  handoff: Handoff;
  /** The clone the change is checked out in. */
  repo: string;
  /** The reading order, with notes, as the CLI sent it. */
  reading: OrderedFile[];
  /** Repo-relative paths the diff shows on its right side, where comments may go. */
  files: string[];
  /** The left side of a file's diff, for threads on removed lines. */
  leftUri: (file: string) => vscode.Uri | undefined;
  /** Opens the diff against a commit, for the changes since the last review. */
  openChangesSinceReview: (sha: string) => Promise<void>;
}

/** Pictures fetched for the description are kept this long, then pruned. */
const PICTURE_DAYS = 30;

type Verdict = "comment" | "approve" | "requestChanges";

export class ReviewSession implements vscode.Disposable {
  private readonly controller: vscode.CommentController;
  private readonly fileSet: Set<string>;
  private readonly storage: vscode.Uri;
  private readonly discussionOf = new WeakMap<vscode.CommentThread, string>();
  private readonly drafts = new Map<vscode.CommentThread, Draft>();
  private draftCounter = 0;
  /** Threads GitLab holds on lines, redrawn on each reload. */
  private lineThreads: vscode.CommentThread[] = [];
  /** The model's notes, drawn once: they do not change while the review is open. */
  private noteThreads: vscode.CommentThread[] = [];
  private panel: vscode.WebviewPanel | undefined;
  private lastReviewed: LastReview | undefined;
  private imagesLoaded = false;
  private readonly model = {
    changeThreads: [] as RemoteThread[],
    approvedBy: [] as string[],
    userHasApproved: false,
    reviewer: undefined as gl.ReviewerState | undefined,
    related: [] as gl.RelatedIssue[],
    errors: [] as string[],
    images: new Map<string, string>(),
  };

  constructor(private readonly options: SessionOptions) {
    this.fileSet = new Set(options.files);
    this.storage = vscode.Uri.joinPath(options.context.globalStorageUri, "pictures");
    this.controller = vscode.comments.createCommentController("changewalk", "changewalk review");
    this.controller.options = { prompt: "Write a comment", placeHolder: "Comment on this line" };
    this.controller.commentingRangeProvider = {
      provideCommentingRanges: (document) => {
        const file = this.relative(document.uri);
        return file && this.fileSet.has(file) ? [new vscode.Range(0, 0, Math.max(document.lineCount - 1, 0), 0)] : [];
      },
    };
  }

  /** Opens the description, notes and threads. The diff is already open. */
  async start(): Promise<void> {
    this.lastReviewed = this.options.context.globalState.get<LastReview>(this.reviewKey());
    this.panel = vscode.window.createWebviewPanel(
      "changewalk.description",
      `${this.number()} · ${this.options.handoff.title}`,
      { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
      { enableScripts: true, localResourceRoots: [this.storage] },
    );
    this.panel.onDidDispose(() => (this.panel = undefined));
    this.panel.webview.onDidReceiveMessage((message) => void this.handleMessage(message).catch(report));
    this.showNotes();
    await this.reload();
  }

  /** Re-reads threads, approvals, standing and related issues from GitLab, then redraws. */
  async reload(): Promise<void> {
    this.model.errors = [];
    if (this.options.handoff.forge !== "gitlab") {
      this.model.errors.push("Threads and verdicts need a GitLab merge request; this change is on GitHub.");
      this.renderDescription();
      return;
    }
    const { repo } = this.options;
    const iid = this.options.handoff.iid;
    const [threads, approvals, reviewer, related] = await Promise.all([
      this.settle("threads", gl.fetchThreads(repo, iid)),
      this.settle("approvals", gl.fetchApprovals(repo, iid)),
      this.settle("your standing", gl.fetchReviewerState(repo, iid)),
      this.settle("related issues", gl.fetchRelatedIssues(repo, iid)),
    ]);
    if (threads) {
      this.showLineThreads(threads.onLines);
      this.model.changeThreads = threads.onChange;
    }
    if (approvals) {
      this.model.approvedBy = approvals.approvedBy;
      this.model.userHasApproved = approvals.userHasApproved;
    }
    this.model.reviewer = reviewer;
    this.model.related = related ?? [];
    if (!this.imagesLoaded) {
      await this.loadPictures();
    }
    this.renderDescription();
  }

  /** A reply from a comment box: to a thread on GitLab, or as the draft of a new one. */
  async reply(reply: vscode.CommentReply): Promise<void> {
    const text = reply.text.trim();
    if (!text) {
      return;
    }
    const thread = reply.thread;
    const discussionId = this.discussionOf.get(thread);
    if (discussionId) {
      await gl.replyToThread(this.options.repo, this.options.handoff.iid, discussionId, text);
      await this.reload();
      return;
    }
    const existing = this.drafts.get(thread);
    if (existing) {
      existing.body = `${existing.body}\n\n${text}`;
      thread.comments = [this.draftComment(existing.body)];
      return;
    }
    const file = this.relative(thread.uri);
    if (!file || !this.fileSet.has(file) || !thread.range) {
      throw new Error("comments go on the lines of the changed files only");
    }
    const draft: Draft = {
      id: String(++this.draftCounter),
      path: file,
      startLine: thread.range.start.line + 1,
      line: thread.range.end.line + 1,
      body: text,
    };
    this.drafts.set(thread, draft);
    thread.comments = [this.draftComment(text)];
    thread.canReply = true;
    thread.contextValue = "draft";
    thread.collapsibleState = vscode.CommentThreadCollapsibleState.Expanded;
  }

  /** Drops a draft that has not been posted. */
  discardDraft(thread: vscode.CommentThread): void {
    if (this.drafts.delete(thread)) {
      thread.dispose();
    }
  }

  /** Marks a thread resolved, or open again. */
  async toggleResolved(thread: vscode.CommentThread): Promise<void> {
    const discussionId = this.discussionOf.get(thread);
    if (!discussionId) {
      return;
    }
    const resolved = thread.state === vscode.CommentThreadState.Resolved;
    await gl.setThreadResolved(this.options.repo, this.options.handoff.iid, discussionId, !resolved);
    await this.reload();
  }

  /** Edits one of the user's posted comments. Only comments GitLab holds can be edited. */
  async editComment(comment: vscode.Comment): Promise<void> {
    const match = /^remote:([^:]+):(\d+)$/.exec(String(comment.contextValue ?? ""));
    if (!match) {
      throw new Error("only a posted comment can be edited");
    }
    const current = typeof comment.body === "string" ? comment.body : comment.body.value;
    const body = await vscode.window.showInputBox({ prompt: "Edit comment", value: current, ignoreFocusOut: true });
    if (body === undefined || body.trim() === "" || body.trim() === current.trim()) {
      return;
    }
    await gl.updateNote(this.options.repo, this.options.handoff.iid, match[1], Number(match[2]), body.trim());
    await this.reload();
  }

  /** Posts the drafts, then the summary, then the verdict, as the Zed fork's review does. */
  async postReview(): Promise<void> {
    const { handoff, repo } = this.options;
    const refs = handoff.diff_refs;
    if (!refs) {
      throw new Error("the change has no diff refs yet; is the branch pushed?");
    }
    const pick = await vscode.window.showQuickPick<vscode.QuickPickItem & { verdict: Verdict }>(
      [
        { label: "Comment", description: "Post the drafts and summary, no verdict", verdict: "comment" },
        { label: this.model.userHasApproved ? "Withdraw approval" : "Approve", verdict: "approve" },
        { label: this.isRequestingChanges() ? "Withdraw request for changes" : "Request changes", verdict: "requestChanges" },
      ],
      { placeHolder: `Post ${this.drafts.size} draft comment(s) with a verdict`, ignoreFocusOut: true },
    );
    if (!pick) {
      return;
    }
    const summary = await vscode.window.showInputBox({ prompt: "Summary (optional)", ignoreFocusOut: true });
    if (summary === undefined) {
      return;
    }

    const entries = [...this.drafts.entries()];
    const result = await gl.postDrafts(repo, handoff.iid, refs, entries.map(([, draft]) => draft), (file) =>
      loadFileOnGitLab(repo, refs, file),
    );
    for (const [thread, draft] of entries) {
      if (result.posted.includes(draft.id)) {
        this.drafts.delete(thread);
        thread.dispose();
      }
    }

    // A verdict waits while a comment has not been placed, so the review is not approved half-posted.
    const blocked = result.failed.length > 0 && pick.verdict !== "comment";
    if (!blocked) {
      if (summary.trim()) {
        await gl.postNote(repo, handoff.iid, summary.trim());
      }
      if (pick.verdict !== "comment") {
        await this.applyVerdict(pick.verdict);
      }
    }
    await this.reload();

    const posted = result.posted.length;
    const lines = [`Posted ${posted} comment${posted === 1 ? "" : "s"}.`];
    if (blocked) {
      lines.push("The verdict was not given while comments are unplaced; fix them and post again.");
    }
    if (result.failed.length > 0) {
      lines.push(...result.failed.map((failure) => `Not posted: ${failure.error}`));
      void vscode.window.showWarningMessage(lines.join("\n"), { modal: false });
    } else {
      void vscode.window.showInformationMessage(lines.join(" "));
    }
  }

  /** Gives or withdraws approval, or the request for changes, with no comments. */
  async verdict(kind: "approve" | "requestChanges"): Promise<void> {
    await this.applyVerdict(kind);
    await this.reload();
  }

  /** Records the commit now at the tip as reviewed, so the next visit can show what changed. */
  async markReviewed(): Promise<void> {
    const headSha = this.options.handoff.diff_refs?.head_sha;
    if (!headSha) {
      throw new Error("the change has no commit to mark; is the branch pushed?");
    }
    const review: LastReview = { headSha, at: new Date().toISOString() };
    await this.options.context.globalState.update(this.reviewKey(), review);
    this.lastReviewed = review;
    this.renderDescription();
  }

  dispose(): void {
    this.controller.dispose();
    this.panel?.dispose();
  }

  // ---- internals -----------------------------------------------------------

  private async applyVerdict(verdict: "approve" | "requestChanges"): Promise<void> {
    const { repo, handoff } = this.options;
    if (verdict === "approve") {
      await gl.setApproval(repo, handoff.iid, !this.model.userHasApproved);
      return;
    }
    const projectPath = gl.projectPathFromUrl(handoff.web_url);
    if (!projectPath) {
      throw new Error("the merge request's project path is unknown");
    }
    await gl.setChangesRequested(repo, projectPath, handoff.iid, !this.isRequestingChanges());
  }

  private isRequestingChanges(): boolean {
    return this.model.reviewer === "requested_changes";
  }

  private async handleMessage(message: { type: string; [key: string]: unknown }): Promise<void> {
    const { repo, handoff } = this.options;
    switch (message.type) {
      case "open": {
        const file = path.join(repo, String(message.path));
        await vscode.window.showTextDocument(vscode.Uri.file(file), { preview: false });
        return;
      }
      case "markReviewed":
        await this.markReviewed();
        return;
      case "changesSinceReview":
        if (this.lastReviewed) {
          await this.options.openChangesSinceReview(this.lastReviewed.headSha);
        }
        return;
      case "startThread": {
        const text = String(message.text ?? "").trim();
        if (text) {
          await gl.startThread(repo, handoff.iid, text);
          await this.reload();
        }
        return;
      }
      case "reply": {
        const text = String(message.text ?? "").trim();
        if (text) {
          await gl.replyToThread(repo, handoff.iid, String(message.id), text);
          await this.reload();
        }
        return;
      }
      case "resolve":
        await gl.setThreadResolved(repo, handoff.iid, String(message.id), Boolean(message.resolved));
        await this.reload();
        return;
    }
  }

  private showNotes(): void {
    for (const file of this.options.reading) {
      if (!this.fileSet.has(file.path)) {
        continue;
      }
      for (const note of file.notes ?? []) {
        const uri = vscode.Uri.file(path.join(this.options.repo, file.path));
        const thread = this.controller.createCommentThread(uri, lineRange(note.line, note.line), [
          { body: new vscode.MarkdownString(note.text), mode: vscode.CommentMode.Preview, author: { name: "changewalk" } },
        ]);
        thread.canReply = false;
        thread.contextValue = "note";
        thread.collapsibleState = vscode.CommentThreadCollapsibleState.Expanded;
        this.noteThreads.push(thread);
      }
    }
  }

  private showLineThreads(threads: RemoteThread[]): void {
    for (const thread of this.lineThreads) {
      thread.dispose();
    }
    this.lineThreads = [];
    for (const remote of threads) {
      if (!remote.path || remote.line === undefined) {
        continue;
      }
      const uri = remote.oldSide
        ? this.options.leftUri(remote.path)
        : vscode.Uri.file(path.join(this.options.repo, remote.path));
      if (!uri) {
        continue;
      }
      const thread = this.controller.createCommentThread(
        uri,
        lineRange(remote.startLine ?? remote.line, remote.line),
        remote.notes.map((note) => this.remoteComment(note, remote.id)),
      );
      thread.canReply = true;
      thread.state = remote.resolved ? vscode.CommentThreadState.Resolved : vscode.CommentThreadState.Unresolved;
      thread.collapsibleState = remote.resolved
        ? vscode.CommentThreadCollapsibleState.Collapsed
        : vscode.CommentThreadCollapsibleState.Expanded;
      thread.contextValue = "remote";
      this.discussionOf.set(thread, remote.id);
      this.lineThreads.push(thread);
    }
  }

  private remoteComment(note: RemoteNote, discussionId: string): vscode.Comment {
    return {
      body: new vscode.MarkdownString(note.body),
      mode: vscode.CommentMode.Preview,
      author: { name: note.author || "unknown" },
      timestamp: note.createdAt ? new Date(note.createdAt) : undefined,
      contextValue: note.id !== undefined ? `remote:${discussionId}:${note.id}` : undefined,
    };
  }

  private draftComment(text: string): vscode.Comment {
    return {
      body: new vscode.MarkdownString(text),
      mode: vscode.CommentMode.Preview,
      author: { name: "Draft" },
      contextValue: "draft",
    };
  }

  private renderDescription(): void {
    if (!this.panel) {
      return;
    }
    const { handoff } = this.options;
    const head = handoff.diff_refs?.head_sha;
    const model: DescriptionModel = {
      handoff,
      approvedBy: this.model.approvedBy,
      reviewer: this.model.reviewer,
      related: this.model.related,
      reading: this.options.reading,
      changeThreads: this.model.changeThreads,
      errors: this.model.errors,
      lastReviewed:
        this.lastReviewed && head && this.lastReviewed.headSha !== head
          ? { headSha: this.lastReviewed.headSha, headNow: head }
          : undefined,
      imageSrc: (link) => this.imageSrc(link),
      nonce: randomBytes(16).toString("base64"),
      cspSource: this.panel.webview.cspSource,
    };
    this.panel.webview.html = descriptionHtml(model);
  }

  private imageSrc(link: string): string | undefined {
    const upload = gl.uploadFromLink(link, this.projectUrl());
    if (upload) {
      return this.model.images.get(uploadKey(upload));
    }
    return /^https?:\/\//.test(link) ? link : undefined;
  }

  /** Fetches the pictures the description links to, once, and keeps them for a while. */
  private async loadPictures(): Promise<void> {
    this.imagesLoaded = true;
    const description = this.options.handoff.description ?? "";
    const uploads = gl.uploadsIn(description).filter(gl.isImage);
    if (uploads.length === 0 || !this.panel) {
      return;
    }
    await fs.mkdir(this.storage.fsPath, { recursive: true });
    await pruneOlderThan(this.storage.fsPath, PICTURE_DAYS);
    for (const upload of uploads) {
      const file = path.join(this.storage.fsPath, gl.uploadCacheName(upload));
      try {
        await fs.access(file);
      } catch {
        try {
          const bytes = await gl.fetchUpload(this.options.repo, upload);
          const partial = `${file}.part`;
          await fs.writeFile(partial, bytes);
          await fs.rename(partial, file);
        } catch (error) {
          this.model.errors.push(`Picture ${upload.fileName} did not load: ${errorText(error)}`);
          continue;
        }
      }
      this.model.images.set(uploadKey(upload), this.panel.webview.asWebviewUri(vscode.Uri.file(file)).toString());
    }
  }

  private async settle<T>(what: string, promise: Promise<T>): Promise<T | undefined> {
    try {
      return await promise;
    } catch (error) {
      this.model.errors.push(`Could not read ${what}: ${errorText(error)}`);
      return undefined;
    }
  }

  private projectUrl(): string | undefined {
    return this.options.handoff.web_url.split("/-/")[0];
  }

  private number(): string {
    const prefix = this.options.handoff.forge === "github" ? "#" : "!";
    return `${prefix}${this.options.handoff.iid}`;
  }

  private reviewKey(): string {
    return `changewalk.lastReview:${this.options.handoff.web_url}`;
  }

  /** The file's path relative to the clone, with forward slashes, for files on disk. */
  private relative(uri: vscode.Uri): string | undefined {
    if (uri.scheme !== "file") {
      return undefined;
    }
    return path.relative(this.options.repo, uri.fsPath).split(path.sep).join("/");
  }
}

function lineRange(start: number, end: number): vscode.Range {
  return new vscode.Range(start - 1, 0, Math.max(end, start) - 1, 0);
}

function uploadKey(upload: gl.Upload): string {
  return `/uploads/${upload.secret}/${upload.fileName}`;
}

async function pruneOlderThan(dir: string, days: number): Promise<void> {
  const cutoff = Date.now() - days * 86_400_000;
  for (const name of await fs.readdir(dir)) {
    const file = path.join(dir, name);
    const stat = await fs.stat(file);
    if (stat.mtimeMs < cutoff) {
      await fs.rm(file, { force: true });
    }
  }
}

function report(error: unknown): void {
  void vscode.window.showErrorMessage(`changewalk: ${errorText(error)}`);
}

