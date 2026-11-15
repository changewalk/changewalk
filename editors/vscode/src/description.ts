// The description panel as HTML: the change's header, approvals, related
// issues, the reading order, the description with its pictures, summary, and
// the threads on the change itself. Pure, so it can be tested without VS Code.

import { Handoff, OrderedFile, changeNumber } from "./handoff";
import { RelatedIssue, RemoteThread, ReviewerState } from "./gitlab";
import { formatNoteTime } from "./time";

export interface DescriptionModel {
  handoff: Handoff;
  approvedBy: string[];
  reviewer: ReviewerState | undefined;
  related: RelatedIssue[];
  reading: OrderedFile[];
  changeThreads: RemoteThread[];
  /** Problems reading from GitLab, shown rather than hidden. */
  errors: string[];
  /** The commit reviewed last, and the tip now, when they differ. */
  lastReviewed?: { headSha: string; headNow: string };
  /** Maps a link in the description to where its picture is drawn from. */
  imageSrc: (link: string) => string | undefined;
  nonce: string;
  cspSource: string;
}

export function descriptionHtml(model: DescriptionModel): string {
  const { handoff } = model;
  const author = handoff.author?.name ? ` · ${escapeHtml(handoff.author.name)}` : "";
  const counts =
    handoff.additions !== undefined || handoff.deletions !== undefined
      ? `<p class="counts">+${handoff.additions ?? 0} −${handoff.deletions ?? 0}</p>`
      : "";
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${model.cspSource} https: data:; style-src 'unsafe-inline'; script-src 'nonce-${model.nonce}';">
<style>${STYLE}</style>
</head>
<body>
  <h1>${escapeHtml(handoff.title)}</h1>
  <p class="meta">${escapeHtml(changeNumber(handoff))}${author} · <a href="${escapeAttr(handoff.web_url)}">open on host</a></p>
  ${counts}
  ${standing(model)}
  ${lastReviewed(model)}
  ${handoff.review?.summary ? `<section class="summary"><h2>Summary</h2><p>${escapeHtml(handoff.review.summary)}</p></section>` : ""}
  ${readingOrder(model.reading)}
  ${relatedIssues(model.related)}
  <section><h2>Description</h2>${
    handoff.description
      ? `<div class="body">${renderMarkdown(handoff.description, model.imageSrc)}</div>`
      : `<p class="empty">No description.</p>`
  }</section>
  ${threadsSection(model.changeThreads)}
  ${model.errors.map((error) => `<p class="error">${escapeHtml(error)}</p>`).join("\n  ")}
  <script nonce="${model.nonce}">${SCRIPT}</script>
</body>
</html>`;
}

function standing(model: DescriptionModel): string {
  const parts: string[] = [];
  switch (model.reviewer) {
    case "approved":
      parts.push("You approved");
      break;
    case "requested_changes":
      parts.push("You requested changes");
      break;
    case "reviewed":
      parts.push("You reviewed");
      break;
    default:
      break;
  }
  if (model.approvedBy.length > 0) {
    parts.push(`Approved by ${model.approvedBy.map(escapeHtml).join(", ")}`);
  }
  const text = parts.length > 0 ? parts.join(" · ") : "Not approved yet";
  return `<p class="standing">${text}</p>`;
}

function lastReviewed(model: DescriptionModel): string {
  const last = model.lastReviewed;
  if (!last) {
    return `<p class="since"><button data-action="markReviewed">Mark as reviewed</button></p>`;
  }
  return `<p class="since">Reviewed at ${escapeHtml(last.headSha.slice(0, 8))}; the change is now at ${escapeHtml(
    last.headNow.slice(0, 8),
  )}. <button data-action="changesSinceReview">Show changes since review</button> <button data-action="markReviewed">Mark as reviewed</button></p>`;
}

function readingOrder(reading: OrderedFile[]): string {
  if (reading.length === 0) {
    return "";
  }
  const items = reading
    .map((file, index) => {
      const notes = file.notes?.length ?? 0;
      const reason = file.reason ? ` <span class="reason">${escapeHtml(file.reason)}</span>` : "";
      const count = notes > 0 ? ` <span class="count">${notes} note${notes === 1 ? "" : "s"}</span>` : "";
      return `<li><a href="#" data-action="open" data-path="${escapeAttr(file.path)}">${index + 1}. ${escapeHtml(
        file.path,
      )}</a>${count}${reason}</li>`;
    })
    .join("\n");
  return `<section><h2>Reading order</h2><ol class="order">${items}</ol></section>`;
}

function relatedIssues(issues: RelatedIssue[]): string {
  if (issues.length === 0) {
    return "";
  }
  const items = issues
    .map((issue) => `<li><a href="${escapeAttr(issue.web_url)}">#${issue.iid} ${escapeHtml(issue.title)}</a></li>`)
    .join("");
  return `<section><h2>Related issues</h2><ul>${items}</ul></section>`;
}

function threadsSection(threads: RemoteThread[]): string {
  const cards = threads
    .map((thread) => {
      const id = escapeAttr(thread.id);
      const notes = thread.notes
        .map(
          (note) =>
            `<div class="note"><div class="head"><b>${escapeHtml(note.author)}</b>${
              note.createdAt ? ` <span class="time">${escapeHtml(formatNoteTime(note.createdAt))}</span>` : ""
            }</div><div class="body">${renderMarkdown(note.body, () => undefined)}</div></div>`,
        )
        .join("");
      return `<div class="thread${thread.resolved ? " resolved" : ""}">${notes}
        <div class="reply"><textarea id="reply-${id}" rows="2" placeholder="Reply"></textarea>
        <button data-action="reply" data-id="${id}">Reply</button>
        <button data-action="resolve" data-id="${id}" data-resolved="${thread.resolved ? "false" : "true"}">${
          thread.resolved ? "Reopen" : "Resolve"
        }</button></div></div>`;
    })
    .join("\n");
  return `<section class="threads"><h2>Threads on the change</h2>${cards || `<p class="empty">No threads on the change.</p>`}
    <div class="reply"><textarea id="new-thread" rows="2" placeholder="Start a thread on the change"></textarea>
    <button data-action="startThread">Start thread</button></div></section>`;
}

/** A small markdown reader: pictures and links in a line, the rest as text, line breaks kept. */
export function renderMarkdown(text: string, imageSrc: (link: string) => string | undefined): string {
  const token = /!\[([^\]]*)\]\(([^)\s]+)\)|\[([^\]]+)\]\(([^)\s]+)\)/g;
  let html = "";
  let last = 0;
  for (const match of text.matchAll(token)) {
    const index = match.index ?? 0;
    html += escapeHtml(text.slice(last, index));
    if (match[1] !== undefined) {
      const src = imageSrc(match[2]);
      html += src ? `<img src="${escapeAttr(src)}" alt="${escapeAttr(match[1])}">` : escapeHtml(match[0]);
    } else {
      const href = match[4];
      html += /^https?:\/\//.test(href) ? `<a href="${escapeAttr(href)}">${escapeHtml(match[3])}</a>` : escapeHtml(match[0]);
    }
    last = index + match[0].length;
  }
  return html + escapeHtml(text.slice(last));
}

export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => {
    switch (c) {
      case "&":
        return "&amp;";
      case "<":
        return "&lt;";
      case ">":
        return "&gt;";
      case '"':
        return "&quot;";
      default:
        return "&#39;";
    }
  });
}

function escapeAttr(text: string): string {
  return escapeHtml(text);
}

const STYLE = `
  body { font-family: var(--vscode-font-family); padding: 1rem 1.25rem; line-height: 1.5; }
  h1 { font-size: 1.1rem; margin: 0 0 .25rem; }
  h2 { font-size: .95rem; margin: 1.25rem 0 .5rem; }
  .meta, .counts, .time, .count, .reason, .empty { color: var(--vscode-descriptionForeground); }
  .counts { margin: 0 0 .5rem; }
  .standing, .since { margin: .25rem 0 .75rem; }
  .body { white-space: pre-wrap; word-wrap: break-word; }
  .body img { max-width: 100%; }
  .error { color: var(--vscode-errorForeground); }
  a { color: var(--vscode-textLink-foreground); }
  .summary { border-left: 2px solid var(--vscode-focusBorder); padding-left: .75rem; }
  ol.order { padding-left: 1.25rem; }
  ol.order li { margin-bottom: .25rem; }
  .thread { border-left: 2px solid var(--vscode-focusBorder); padding: .25rem 0 .25rem .75rem; margin-bottom: 1rem; }
  .thread.resolved { opacity: .6; }
  .note { margin-bottom: .5rem; }
  .reply { display: flex; flex-wrap: wrap; gap: .25rem; margin-top: .25rem; }
  .reply textarea { flex: 1 1 100%; font-family: inherit; background: var(--vscode-input-background); color: var(--vscode-input-foreground); border: 1px solid var(--vscode-input-border, transparent); }
  button { background: var(--vscode-button-background); color: var(--vscode-button-foreground); border: none; padding: .25rem .6rem; cursor: pointer; }
`;

const SCRIPT = `
  const vscode = acquireVsCodeApi();
  document.addEventListener('click', (event) => {
    const target = event.target.closest('[data-action]');
    if (!target) return;
    event.preventDefault();
    const id = target.dataset.id;
    switch (target.dataset.action) {
      case 'open': vscode.postMessage({ type: 'open', path: target.dataset.path }); break;
      case 'markReviewed': vscode.postMessage({ type: 'markReviewed' }); break;
      case 'changesSinceReview': vscode.postMessage({ type: 'changesSinceReview' }); break;
      case 'startThread': vscode.postMessage({ type: 'startThread', text: document.getElementById('new-thread').value }); break;
      case 'reply': vscode.postMessage({ type: 'reply', id, text: document.getElementById('reply-' + id).value }); break;
      case 'resolve': vscode.postMessage({ type: 'resolve', id, resolved: target.dataset.resolved === 'true' }); break;
    }
  });
`;
