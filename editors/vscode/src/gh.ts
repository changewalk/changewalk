// Runs `gh api` in the clone, so requests use the user's GitHub login. The
// GitHub counterpart to glab.ts; `--repo` is passed by the caller as a flag in
// the endpoint args when the clone's origin is not the review's repo.

import { spawn } from "child_process";

export class GhError extends Error {}

interface Reply {
  stdout: Buffer;
  stderr: string;
  code: number | null;
}

function run(cwd: string, args: string[], input?: string): Promise<Reply> {
  return new Promise((resolve, reject) => {
    const child = spawn("gh", args, { cwd, stdio: [input === undefined ? "ignore" : "pipe", "pipe", "pipe"] });
    const out: Buffer[] = [];
    const err: Buffer[] = [];
    child.stdout!.on("data", (chunk: Buffer) => out.push(chunk));
    child.stderr!.on("data", (chunk: Buffer) => err.push(chunk));
    child.on("error", () => reject(new GhError("could not run `gh`; install it and run `gh auth login`")));
    child.on("close", (code) =>
      resolve({ stdout: Buffer.concat(out), stderr: Buffer.concat(err).toString("utf8"), code }),
    );
    if (input !== undefined) {
      child.stdin!.end(input);
    }
  });
}

/** The raw response of `gh api`, bytes as they came. A JSON body is sent on
 *  stdin so values with newlines or quotes survive. */
export async function ghApiBytes(cwd: string, method: string, endpoint: string, body?: unknown): Promise<Buffer> {
  const args = ["api", "--method", method, endpoint];
  if (body !== undefined) {
    args.push("--input", "-");
  }
  const reply = await run(cwd, args, body === undefined ? undefined : JSON.stringify(body));
  if (reply.code !== 0) {
    const detail = [reply.stderr.trim(), reply.stdout.toString("utf8").trim()].filter(Boolean).join("\n");
    throw new GhError(`\`gh api ${endpoint}\` failed: ${detail}`);
  }
  return reply.stdout;
}

/** The text of `gh api`, for JSON replies. */
export async function ghApi(cwd: string, method: string, endpoint: string, body?: unknown): Promise<string> {
  return (await ghApiBytes(cwd, method, endpoint, body)).toString("utf8");
}

/** Runs a GraphQL query with variables and returns the reply text. Variables
 *  are sent in the body so ids and multi-line bodies need no escaping. */
export async function ghGraphql(cwd: string, query: string, variables: Record<string, unknown> = {}): Promise<string> {
  const reply = await run(cwd, ["api", "graphql", "--input", "-"], JSON.stringify({ query, variables }));
  if (reply.code !== 0) {
    const detail = [reply.stderr.trim(), reply.stdout.toString("utf8").trim()].filter(Boolean).join("\n");
    throw new GhError(`\`gh api graphql\` failed: ${detail}`);
  }
  return reply.stdout.toString("utf8");
}
