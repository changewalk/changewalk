// VS Code's built-in Git extension, the one piece of git access that needs
// the editor: a `git:` URI for a file's content at a ref, so the diff view
// can show base vs working tree. The pure git reads live in review.ts.

import * as vscode from "vscode";

/** The minimal slice of VS Code's Git extension API we use. */
interface GitApi {
  toGitUri(uri: vscode.Uri, ref: string): vscode.Uri;
}

/** The built-in Git extension, present in both VS Code and Cursor. */
export async function activateGitExtension(): Promise<GitApi | undefined> {
  const extension = vscode.extensions.getExtension<{ getAPI(version: 1): GitApi }>("vscode.git");
  if (!extension) {
    return undefined;
  }
  if (!extension.isActive) {
    await extension.activate();
  }
  return extension.exports.getAPI(1);
}
