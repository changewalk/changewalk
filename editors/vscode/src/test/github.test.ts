// Headless tests for the GitHub parsing: reading threads, approvals, standing,
// the pull request's parts off a URL, and placing a draft on the diff. The gh
// calls themselves are covered by driving the extension in VS Code and Cursor.

import { test } from "node:test";
import assert from "node:assert/strict";

import {
  approvalsFrom,
  changeThreadsFrom,
  latestReviewStates,
  lineThreadsFrom,
  placeGithubComment,
  pullRefFromUrl,
  reviewerStateFrom,
} from "../github";
import { parseDiffFile, parseHunks, FileOnGitLab } from "../placement";
import { Draft } from "../gitlab";
import { DiffRefs } from "../handoff";

test("pullRefFromUrl reads the owner, name and number off a pull request URL", () => {
  assert.deepEqual(pullRefFromUrl("https://github.com/changewalk/changewalk/pull/3", 0), {
    owner: "changewalk",
    name: "changewalk",
    number: 3,
  });
  // The number falls back when the URL does not carry one (a repo page).
  assert.deepEqual(pullRefFromUrl("https://github.com/acme/widget", 42), {
    owner: "acme",
    name: "widget",
    number: 42,
  });
  assert.equal(pullRefFromUrl("not a url", 1), undefined);
});

test("lineThreadsFrom reads the path, line and side off the thread, not its comments", () => {
  const { threads, routing } = lineThreadsFrom([
    {
      id: "T_kwR",
      isResolved: false,
      path: "src/a.ts",
      line: 12,
      startLine: 10,
      diffSide: "RIGHT",
      comments: { nodes: [{ databaseId: 111, body: "first", createdAt: "2026-11-16T10:00:00Z", author: { login: "jack" } }] },
    },
    {
      // A thread on a removed line sits on the old side; a stale line falls back to the original.
      id: "T_old",
      isResolved: true,
      path: "src/b.ts",
      line: null,
      originalLine: 5,
      diffSide: "LEFT",
      comments: { nodes: [{ databaseId: 222, body: "gone", author: { login: "amy" } }] },
    },
  ]);
  assert.equal(threads.length, 2);
  assert.deepEqual(
    { path: threads[0].path, line: threads[0].line, startLine: threads[0].startLine, oldSide: threads[0].oldSide, resolved: threads[0].resolved },
    { path: "src/a.ts", line: 12, startLine: 10, oldSide: false, resolved: false },
  );
  assert.deepEqual({ line: threads[1].line, oldSide: threads[1].oldSide, resolved: threads[1].resolved }, { line: 5, oldSide: true, resolved: true });
  // The routing remembers the node id to resolve by and the root comment to reply under.
  assert.deepEqual(routing.get("T_kwR"), { nodeId: "T_kwR", rootCommentId: 111 });
});

test("changeThreadsFrom turns pull request comments into unresolvable change threads", () => {
  const { threads, routing } = changeThreadsFrom([
    { id: 900, body: "looks good overall", created_at: "2026-11-16T09:00:00Z", user: { login: "jack" } },
  ]);
  assert.equal(threads.length, 1);
  assert.equal(threads[0].id, "issue:900");
  assert.equal(threads[0].resolvable, false);
  assert.equal(threads[0].notes[0].author, "jack");
  assert.deepEqual(routing.get("issue:900"), { issueComment: true });
});

test("latestReviewStates keeps the newest verdict and lets a later comment not overturn it", () => {
  const states = latestReviewStates([
    { user: { login: "jack" }, state: "CHANGES_REQUESTED" },
    { user: { login: "jack" }, state: "COMMENTED" },
    { user: { login: "amy" }, state: "APPROVED" },
    { user: { login: "sam" }, state: "PENDING" },
  ]);
  assert.equal(states.get("jack"), "CHANGES_REQUESTED");
  assert.equal(states.get("amy"), "APPROVED");
  assert.equal(states.has("sam"), false);
});

test("approvalsFrom and reviewerStateFrom read who approved and the user's standing", () => {
  const reviews = [
    { user: { login: "amy" }, state: "APPROVED" },
    { user: { login: "jack" }, state: "CHANGES_REQUESTED" },
  ];
  assert.deepEqual(approvalsFrom(reviews, "jack"), { approvedBy: ["amy"], userHasApproved: false });
  assert.deepEqual(approvalsFrom(reviews, "amy"), { approvedBy: ["amy"], userHasApproved: true });
  assert.equal(reviewerStateFrom(reviews, "jack"), "requested_changes");
  assert.equal(reviewerStateFrom(reviews, "amy"), "approved");
  assert.equal(reviewerStateFrom(reviews, "nobody"), "unreviewed");
});

test("placeGithubComment maps a draft onto the head line on the right side", () => {
  // A file whose first three lines the change added; disk matches head.
  const diff = "--- /dev/null\n+++ b/src/a.ts\n@@ -0,0 +1,3 @@\n+one\n+two\n+three\n";
  const file: FileOnGitLab = { path: "src/a.ts", diff: parseDiffFile(diff), sinceHead: parseHunks("") };
  const refs: DiffRefs = { base_sha: "b".repeat(40), head_sha: "h".repeat(40), start_sha: "b".repeat(40) };
  const draft: Draft = { id: "1", path: "src/a.ts", startLine: 2, line: 2, body: "why two?" };
  const placed = placeGithubComment(file, draft, refs);
  assert.ok("comment" in placed);
  assert.deepEqual(placed.comment, { path: "src/a.ts", line: 2, side: "RIGHT", body: "why two?" });

  // A multi-line draft carries the start line on the same side.
  const span = placeGithubComment(file, { ...draft, startLine: 1, line: 3 }, refs);
  assert.ok("comment" in span);
  assert.equal(span.comment.start_line, 1);
  assert.equal(span.comment.start_side, "RIGHT");

  // A line outside the diff is reported, not placed.
  const off = placeGithubComment(file, { ...draft, startLine: 9, line: 9 }, refs);
  assert.ok("error" in off);
  assert.match(off.error, /outside the pull request's diff/);
});
