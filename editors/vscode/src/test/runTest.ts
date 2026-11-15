// Downloads a throwaway VS Code, builds a fixture review, and runs the
// integration suite inside the real editor. The suite (suite/index.js)
// activates the extension and asserts the review opened.

import * as path from "path";
import * as os from "os";
import * as fs from "fs";
import { execFileSync } from "child_process";
import { runTests } from "@vscode/test-electron";

function git(cwd: string, args: string[]): void {
  execFileSync("git", args, { cwd, stdio: "ignore" });
}

/** A clone with an `origin`, a base commit on `main`, and a `feature`
 *  branch (checked out) that adds and changes a file. Returns the clone and
 *  the hand-off file the CLI shape would write. */
function makeFixture(): { repo: string; handoff: string } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "cw-int-"));
  const remote = path.join(dir, "remote.git");
  const repo = path.join(dir, "clone");
  fs.mkdirSync(remote);
  git(remote, ["init", "-q", "--bare", "-b", "main"]);
  fs.mkdirSync(repo);
  git(repo, ["init", "-q", "-b", "main"]);
  git(repo, ["config", "user.email", "t@example.com"]);
  git(repo, ["config", "user.name", "Test"]);
  fs.writeFileSync(path.join(repo, "base.txt"), "base\n");
  git(repo, ["add", "."]);
  git(repo, ["commit", "-q", "-m", "base"]);
  git(repo, ["checkout", "-q", "-b", "feature"]);
  fs.writeFileSync(path.join(repo, "added.txt"), "new\n");
  fs.writeFileSync(path.join(repo, "base.txt"), "base changed\n");
  git(repo, ["add", "."]);
  git(repo, ["commit", "-q", "-m", "feature work"]);
  git(repo, ["remote", "add", "origin", remote]);
  git(repo, ["push", "-q", "origin", "main", "feature"]);

  const handoff = path.join(dir, "handoff.json");
  fs.writeFileSync(
    handoff,
    JSON.stringify({
      iid: 42,
      title: "A change",
      description: "A test review.",
      forge: "gitlab",
      source_branch: "feature",
      target_branch: "main",
      web_url: "https://example.com/x/-/merge_requests/42",
    }),
  );
  return { repo, handoff };
}

async function main(): Promise<void> {
  const extensionDevelopmentPath = path.resolve(__dirname, "../..");
  const extensionTestsPath = path.resolve(__dirname, "./suite/index");
  // CW_E2E_REPO and CW_E2E_HANDOFF point the test at a real clone and the
  // hand-off the CLI wrote, so the same assertions run against a live PR.
  const { repo, handoff } =
    process.env.CW_E2E_REPO && process.env.CW_E2E_HANDOFF
      ? { repo: process.env.CW_E2E_REPO, handoff: process.env.CW_E2E_HANDOFF }
      : makeFixture();
  try {
    await runTests({
      // CW_VSCODE_EXEC runs the test host on another VS Code build (e.g. Cursor).
      ...(process.env.CW_VSCODE_EXEC ? { vscodeExecutablePath: process.env.CW_VSCODE_EXEC } : {}),
      extensionDevelopmentPath,
      extensionTestsPath,
      // Open the clone as the workspace; disable other installed extensions
      // so the test runs against this source, not the installed build.
      // --no-sandbox is needed on Linux CI, where an AppImage-extracted
      // editor has no SUID sandbox helper (set CW_NO_SANDBOX there).
      launchArgs: [repo, "--disable-extensions", ...(process.env.CW_NO_SANDBOX ? ["--no-sandbox"] : [])],
      extensionTestsEnv: { CW_TEST_HANDOFF: handoff },
    });
  } catch (error) {
    console.error("integration tests failed:", error);
    process.exit(1);
  }
}

void main();
