// Runs inside a real VS Code. Opens a review from the hand-off the CLI wrote
// (synthetic fixture, or a live PR via CW_E2E_*) and asserts the diff and the
// description both opened, with tabs that match the change the hand-off names.

import * as assert from "assert";
import * as fs from "fs";
import * as vscode from "vscode";

describe("changewalk review in the editor", () => {
  it("opens the description and the diff for a hand-off", async () => {
    const extension = vscode.extensions.getExtension("changewalk.changewalk");
    assert.ok(extension, "the changewalk extension is not present");
    await extension.activate();

    const handoffPath = process.env.CW_TEST_HANDOFF;
    assert.ok(handoffPath, "CW_TEST_HANDOFF is not set");
    const handoff = JSON.parse(fs.readFileSync(handoffPath, "utf8")) as { iid: number; forge: string };
    const number = `${handoff.forge === "github" ? "#" : "!"}${handoff.iid}`;

    await vscode.commands.executeCommand("changewalk.openReviewFromFile", handoffPath);
    // The command awaits the open, but the editor registers tabs on the next
    // tick; give it a moment before reading them.
    await new Promise((resolve) => setTimeout(resolve, 1500));

    const labels = vscode.window.tabGroups.all.flatMap((group) => group.tabs).map((tab) => tab.label);
    assert.ok(
      labels.some((label) => label.startsWith(number)),
      `no description tab for ${number}; open tabs: ${labels.join(" | ")}`,
    );
    assert.ok(
      labels.some((label) => label.startsWith(`Review ${number}`)),
      `no diff tab for ${number}; open tabs: ${labels.join(" | ")}`,
    );
  });
});
