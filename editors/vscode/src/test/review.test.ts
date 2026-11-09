// Headless tests for the review logic that does not touch the VS Code API.
// The editor glue (opening diffs and webviews) is covered by installing the
// extension into VS Code and Cursor; here we test the parts that decide
// what to show.

import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { parseReviewUri, order, changedFiles, resolveBase, fallbackBase } from "../review";
import { changeNumber, readHandoff, Handoff } from "../handoff";

test("parseReviewUri reads repo, base, and mr", () => {
  const launch = parseReviewUri("/review", "repo=%2Ftmp%2Fclone&base=origin%2Fmain&mr=%2Ftmp%2F7.json");
  assert.deepEqual(launch, { mr: "/tmp/7.json", repo: "/tmp/clone", base: "origin/main" });
});

test("parseReviewUri leaves repo and base undefined when absent", () => {
  const launch = parseReviewUri("/review", "mr=%2Ftmp%2F7.json");
  assert.equal(launch.mr, "/tmp/7.json");
  assert.equal(launch.repo, undefined);
  assert.equal(launch.base, undefined);
});

test("parseReviewUri rejects a missing mr and an unknown action", () => {
  assert.throws(() => parseReviewUri("/review", "repo=%2Ftmp"), /no mr=/);
  assert.throws(() => parseReviewUri("/clone", "mr=x"), /unknown changewalk action/);
});

test("fallbackBase diffs against the target branch on the origin", () => {
  assert.equal(fallbackBase("main"), "origin/main");
});

test("order puts the reading order first, then the rest in git order", () => {
  const handoff = {
    review: { order: [{ path: "src/b.ts" }, { path: "gone.ts" }, { path: "src/a.ts" }] },
  } as Handoff;
  assert.deepEqual(order(["src/a.ts", "src/b.ts", "src/c.ts"], handoff), [
    "src/b.ts",
    "src/a.ts",
    "src/c.ts",
  ]);
});

test("order keeps git order when the hand-off carries none", () => {
  assert.deepEqual(order(["a", "b"], {} as Handoff), ["a", "b"]);
});

test("changeNumber uses each host's prefix", () => {
  assert.equal(changeNumber({ iid: 1628, forge: "gitlab" } as Handoff), "!1628");
  assert.equal(changeNumber({ iid: 1628, forge: "github" } as Handoff), "#1628");
});

// --- Against a real git repo ------------------------------------------------

let repo: string;

function git(args: string[]): string {
  return execFileSync("git", args, { cwd: repo, encoding: "utf8" }).trim();
}

before(() => {
  repo = mkdtempSync(join(tmpdir(), "changewalk-review-"));
  git(["init", "-q", "-b", "main"]);
  git(["config", "user.email", "t@example.com"]);
  git(["config", "user.name", "Test"]);
  writeFileSync(join(repo, "base.txt"), "base\n");
  git(["add", "."]);
  git(["commit", "-q", "-m", "base"]);
  git(["checkout", "-q", "-b", "feature"]);
  writeFileSync(join(repo, "added.txt"), "new\n");
  writeFileSync(join(repo, "base.txt"), "base changed\n");
  git(["add", "."]);
  git(["commit", "-q", "-m", "feature work"]);
});

after(() => {
  rmSync(repo, { recursive: true, force: true });
});

test("changedFiles lists the files the branch changed, marking added ones", async () => {
  const files = await changedFiles(repo, "main");
  const byPath = new Map(files.map((file) => [file.path, file.added]));
  assert.deepEqual([...byPath.keys()].sort(), ["added.txt", "base.txt"]);
  assert.equal(byPath.get("added.txt"), true);
  assert.equal(byPath.get("base.txt"), false);
});

test("resolveBase pins the merge base to a sha", async () => {
  const base = await resolveBase(repo, "main");
  assert.match(base, /^[0-9a-f]{40}$/);
  assert.equal(base, git(["rev-parse", "main"]));
});

test("readHandoff parses a hand-off file the CLI shape would write", async () => {
  const file = join(repo, "handoff.json");
  writeFileSync(
    file,
    JSON.stringify({ iid: 42, title: "A change", forge: "gitlab", source_branch: "feature", target_branch: "main", web_url: "https://x" }),
  );
  const handoff = await readHandoff(file);
  assert.equal(handoff.iid, 42);
  assert.equal(changeNumber(handoff), "!42");
});
