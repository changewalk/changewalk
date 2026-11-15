// Headless tests for the pure comment-placement and GitLab-parsing logic:
// the parts that decide where a comment goes and what a thread looks like.

import { test } from "node:test";
import assert from "node:assert/strict";

import { parseHunkHeader, parseDiffFile, parseHunks, lineInHead, placeComment, FileOnGitLab } from "../placement";
import { DiffRefs } from "../handoff";
import {
  threadsFrom,
  parseApprovals,
  reviewerStateFrom,
  uploadFromLink,
  uploadsIn,
  isImage,
  uploadCacheName,
  projectPathFromUrl,
} from "../gitlab";

const README_DIFF = `diff --git a/docs/README.md b/docs/README.md
index 1111111..2222222 100644
--- a/docs/README.md
+++ b/docs/README.md
@@ -38,4 +38,6 @@ Intro
 context one
-gone
+fresh one
+fresh two
 context two
 context three
`;

const refs: DiffRefs = { base_sha: "b".repeat(40), head_sha: "h".repeat(40), start_sha: "s".repeat(40) };

function fileOnGitLab(sinceHead = parseHunks("")): FileOnGitLab {
  return { path: "docs/README.md", diff: parseDiffFile(README_DIFF), sinceHead };
}

test("parseHunkHeader reads a range with and without a count", () => {
  assert.deepEqual(parseHunkHeader("@@ -38,4 +38,6 @@ Intro"), { oldStart: 38, oldLen: 4, newStart: 38, newLen: 6 });
  assert.deepEqual(parseHunkHeader("@@ -5 +7 @@"), { oldStart: 5, oldLen: 1, newStart: 7, newLen: 1 });
  assert.equal(parseHunkHeader("not a hunk"), undefined);
});

test("parseDiffFile tells added lines from unchanged ones", () => {
  const file = parseDiffFile(README_DIFF);
  assert.equal(file.oldPath, "docs/README.md");
  assert.equal(file.lines.get(39)?.oldLine, undefined, "line 39 was added");
  assert.equal(file.lines.get(41)?.oldLine, 40, "line 41 is unchanged, old line 40");
});

test("lineInHead maps a line around local changes, and drops a changed one", () => {
  // One line added at new line 10 (no old counterpart): lines after shift up by one.
  const hunks = parseHunks("@@ -9,0 +10 @@\n@@ -20 +21,0 @@");
  assert.equal(lineInHead(hunks, 5), 5, "before the change, unmoved");
  assert.equal(lineInHead(hunks, 10), undefined, "the added line itself is not in head");
  assert.equal(lineInHead(hunks, 15), 14, "after the add, shifted back by one");
});

test("a comment on an added line goes to the new side", () => {
  const placed = placeComment(fileOnGitLab(), { path: "docs/README.md", startLine: 39, line: 39, body: "Why?" }, refs);
  assert.ok("position" in placed, "the comment is placed");
  assert.equal(placed.position.new_line, 39);
  assert.equal(placed.position.old_line, undefined);
});

test("a comment on an unchanged line carries both sides", () => {
  const placed = placeComment(fileOnGitLab(), { path: "docs/README.md", startLine: 41, line: 41, body: "Why?" }, refs);
  assert.ok("position" in placed);
  assert.equal(placed.position.new_line, 41);
  assert.equal(placed.position.old_line, 40);
});

test("a multi-line comment spans with GitLab line codes", () => {
  const placed = placeComment(fileOnGitLab(), { path: "docs/README.md", startLine: 39, line: 41, body: "Why?" }, refs);
  assert.ok("position" in placed);
  const range = placed.position.line_range as { start: { new_line: number }; end: { new_line: number } };
  assert.equal(range.start.new_line, 39);
  assert.equal(range.end.new_line, 41);
});

test("a comment off the diff, or on a locally-edited line, says why", () => {
  const offDiff = placeComment(fileOnGitLab(), { path: "docs/README.md", startLine: 2, line: 2, body: "x" }, refs);
  assert.ok("error" in offDiff && /outside the merge request's diff/.test(offDiff.error));
  // A local edit at new line 39 means that line is not in the head commit.
  const edited = placeComment(
    fileOnGitLab(parseHunks("@@ -39 +39 @@")),
    { path: "docs/README.md", startLine: 39, line: 39, body: "x" },
    refs,
  );
  assert.ok("error" in edited && /not in the merge request's head commit/.test(edited.error));
});

test("threadsFrom splits line threads from change threads and drops system notes", () => {
  const { onLines, onChange } = threadsFrom([
    {
      id: "a",
      notes: [{ body: "on a line", author: { name: "Ada" }, position: { new_path: "a.ts", new_line: 12 }, resolvable: true }],
    },
    {
      id: "b",
      notes: [{ body: "on a removed line", author: { name: "Bo" }, position: { old_path: "a.ts", old_line: 7 } }],
    },
    { id: "c", notes: [{ body: "on the change", author: { name: "Cy" }, resolvable: true }] },
    { id: "d", notes: [{ body: "pushed a commit", system: true }] },
    { id: "e", notes: [{ body: "plain comment", author: { name: "Em" } }] },
  ]);
  assert.deepEqual(
    onLines.map((t) => [t.id, t.line, t.oldSide]),
    [
      ["a", 12, false],
      ["b", 7, true],
    ],
  );
  assert.deepEqual(
    onChange.map((t) => t.id),
    ["c"],
    "a resolvable thread on the change is kept; a plain comment and a system note are not",
  );
});

test("parseApprovals and reviewerStateFrom read names and the user's standing", () => {
  const approvals = parseApprovals(JSON.stringify({ approved_by: [{ user: { name: "Ada" } }], user_has_approved: true }));
  assert.deepEqual(approvals, { approvedBy: ["Ada"], userHasApproved: true });
  const reviewers = JSON.stringify([{ user: { username: "me" }, state: "requested_changes" }]);
  assert.equal(reviewerStateFrom(reviewers, "me"), "requested_changes");
  assert.equal(reviewerStateFrom(reviewers, "someone"), "unreviewed");
});

test("uploads are told from other links and parsed from the description", () => {
  const secret = "d2e7a29c38a866c962c5cab82b22f0c9";
  assert.deepEqual(uploadFromLink(`/uploads/${secret}/shot.png`), { secret, fileName: "shot.png" });
  assert.deepEqual(
    uploadFromLink(`https://gitlab.example/group/project/uploads/${secret}/shot.png`, "https://gitlab.example/group/project"),
    { secret, fileName: "shot.png" },
  );
  assert.equal(uploadFromLink("https://example.com/image.png"), undefined, "a link elsewhere is not an upload");
  const text = `| ![a](/uploads/${secret}/a.png) | ![b](/uploads/${secret}/b.png) |`;
  assert.deepEqual(
    uploadsIn(text).map((u) => u.fileName),
    ["a.png", "b.png"],
  );
});

test("isImage and uploadCacheName treat pictures and names as the fork does", () => {
  assert.equal(isImage({ secret: "x".repeat(16), fileName: "a.PNG" }), true);
  assert.equal(isImage({ secret: "x".repeat(16), fileName: "clip.mp4" }), false);
  assert.equal(uploadCacheName({ secret: "abcdef0123456789", fileName: "a b/c.png" }), "abcdef0123456789-a_b_c.png");
});

test("projectPathFromUrl reads the group and project off the web URL", () => {
  assert.equal(projectPathFromUrl("https://gitlab.example/group/project/-/merge_requests/7"), "group/project");
  assert.equal(projectPathFromUrl("not a url"), undefined);
});
