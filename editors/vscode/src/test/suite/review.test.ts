// Runs inside a real VS Code. Opens a review from the fixture hand-off and
// asserts the diff and the description both opened as editor tabs.

import * as assert from "assert";
import * as vscode from "vscode";

describe("changewalk review in the editor", () => {
  it("opens the description and the diff for a hand-off", async () => {
    const extension = vscode.extensions.getExtension("changewalk.changewalk");
    assert.ok(extension, "the changewalk extension is not present");
    await extension.activate();

    const handoff = process.env.CW_TEST_HANDOFF;
    assert.ok(handoff, "CW_TEST_HANDOFF is not set");

    await vscode.commands.executeCommand("changewalk.openReviewFromFile", handoff);
    // The command awaits the open, but the editor registers tabs on the next
    // tick; give it a moment before reading them.
    await new Promise((resolve) => setTimeout(resolve, 500));

    const labels = vscode.window.tabGroups.all.flatMap((group) => group.tabs).map((tab) => tab.label);
    assert.ok(
      labels.some((label) => label.startsWith("!42")),
      `no description tab for the change; open tabs: ${labels.join(" | ")}`,
    );
    assert.ok(
      labels.some((label) => label.startsWith("Review !42")),
      `no diff tab for the change; open tabs: ${labels.join(" | ")}`,
    );
  });
});
