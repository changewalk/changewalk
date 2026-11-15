// Headless tests for the description renderer and the small markdown reader.

import { test } from "node:test";
import assert from "node:assert/strict";

import { descriptionHtml, renderMarkdown, DescriptionModel } from "../description";
import { formatNoteTime } from "../time";
import { Handoff } from "../handoff";

const handoff: Handoff = {
  iid: 42,
  title: "A change",
  description: "See ![shot](/uploads/abc/shot.png) and [docs](https://example.com).",
  source_branch: "topic",
  target_branch: "main",
  web_url: "https://gitlab.example/g/p/-/merge_requests/42",
  forge: "gitlab",
  review: { summary: "It does the thing." },
};

function model(over: Partial<DescriptionModel> = {}): DescriptionModel {
  return {
    handoff,
    approvedBy: [],
    reviewer: undefined,
    related: [],
    reading: [{ path: "src/a.ts", reason: "The main change.", notes: [{ line: 3, text: "here" }] }],
    changeThreads: [],
    errors: [],
    imageSrc: (link) => (link.includes("/uploads/") ? "vscode-resource://pic" : undefined),
    nonce: "n",
    cspSource: "vscode-resource:",
    ...over,
  };
}

test("renderMarkdown draws a fetched picture and keeps an unfetched one as text", () => {
  const html = renderMarkdown("![a](/uploads/abc/a.png) and ![b](/uploads/xyz/b.png)", (link) =>
    link.includes("abc") ? "file://a" : undefined,
  );
  assert.match(html, /<img src="file:\/\/a" alt="a">/);
  assert.match(html, /!\[b\]\(\/uploads\/xyz\/b\.png\)/, "the unfetched picture stays as its link text");
});

test("renderMarkdown links only http(s), and escapes the rest", () => {
  assert.match(renderMarkdown("[x](https://e.com)", () => undefined), /<a href="https:\/\/e\.com">x<\/a>/);
  assert.match(renderMarkdown("[x](javascript:alert(1))", () => undefined), /\[x\]\(javascript/);
  assert.match(renderMarkdown("<script>", () => undefined), /&lt;script&gt;/);
});

test("the description shows the reading order, summary, and standing", () => {
  const html = descriptionHtml(model({ approvedBy: ["Ada"], reviewer: "approved" }));
  assert.match(html, /Reading order/);
  assert.match(html, /1\. src\/a\.ts/);
  assert.match(html, /1 note/);
  assert.match(html, /It does the thing\./);
  assert.match(html, /You approved/);
  assert.match(html, /Approved by Ada/);
});

test("the description offers the changes-since-review when the tip has moved", () => {
  const html = descriptionHtml(model({ lastReviewed: { headSha: "a".repeat(40), headNow: "b".repeat(40) } }));
  assert.match(html, /Show changes since review/);
  assert.match(html, /aaaaaaaa/);
});

test("a change thread draws its notes with a reply and resolve control", () => {
  const html = descriptionHtml(
    model({
      changeThreads: [
        { id: "t1", oldSide: false, resolved: false, resolvable: true, notes: [{ author: "Ada", body: "Think again" }] },
      ],
    }),
  );
  assert.match(html, /Think again/);
  assert.match(html, /data-action="reply" data-id="t1"/);
  assert.match(html, /data-action="resolve" data-id="t1" data-resolved="true"/);
});

test("formatNoteTime reads today, yesterday, and an older date", () => {
  const now = new Date("2026-11-15T10:00:00");
  assert.match(formatNoteTime("2026-11-15T08:30:00", now), /^Today at /);
  assert.match(formatNoteTime("2026-11-14T08:30:00", now), /^Yesterday at /);
  assert.match(formatNoteTime("2026-01-02T08:30:00", now), /2026/);
});
