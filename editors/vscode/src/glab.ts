// Runs `glab api` in the clone, so requests use the user's GitLab login and
// `:id` resolves to the project the clone belongs to. Same as the Zed fork.

import { spawn } from "child_process";

export class GlabError extends Error {}

interface Reply {
  stdout: Buffer;
  stderr: string;
  code: number | null;
}

function run(cwd: string, args: string[], input?: string): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const child = spawn("glab", args, { cwd, stdio: [input === undefined ? "ignore" : "pipe", "pipe", "pipe"] });
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    child.stdout!.on("data", (chunk: Buffer) => out.push(chunk));
    child.stderr!.on("data", (chunk: Buffer) => err.push(chunk));
    child.on("error", () =>
      reject(new GlabError("could not run `glab`; install it and run `glab auth login`")),
    );
    child.on("close", (code) =>
      resolve({ stdout: Buffer.concat(out), stderr: Buffer.concat(err).toString("utf8"), code }),
    );
    if (input !== undefined) {
      child.stdin!.end(input);
    }
  });
}

/** The raw response of `glab api`, bytes as they came. */
export async function glabApiBytes(cwd: string, method: string, endpoint: string, body?: unknown): Promise<Buffer> {
  const args = ["api", "--method", method, endpoint];
  // glab sends a raw body with no content type, which GitLab rejects.
  if (body !== undefined) {
    args.push("--header", "Content-Type: application/json", "--input", "-");
  }
  const reply = await run(cwd, args, body === undefined ? undefined : JSON.stringify(body));
  if (reply.code !== 0) {
    const detail = [reply.stderr.trim(), reply.stdout.toString("utf8").trim()].filter(Boolean).join("\n");
    throw new GlabError(`\`glab api ${endpoint}\` failed: ${detail}`);
  }
  return reply.stdout;
}

/** The text of `glab api`, for JSON replies. */
export async function glabApi(cwd: string, method: string, endpoint: string, body?: unknown): Promise<string> {
  return (await glabApiBytes(cwd, method, endpoint, body)).toString("utf8");
}

/** Runs a GraphQL query and returns the reply text. */
export async function glabGraphql(cwd: string, query: string): Promise<string> {
  const reply = await run(cwd, ["api", "graphql", "-f", `query=${query}`]);
  if (reply.code !== 0) {
    throw new GlabError(`\`glab api graphql\` failed: ${reply.stderr.trim()}`);
  }
  return reply.stdout.toString("utf8");
}
