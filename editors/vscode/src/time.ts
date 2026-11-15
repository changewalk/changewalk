// How a note's time reads in the description panel, as the Zed fork shows it:
// "Today at 14:05", "Yesterday at 14:05", else the date and the time.

export function formatNoteTime(iso: string, now: Date = new Date()): string {
  const at = new Date(iso);
  const time = at.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const dayDiff = Math.round((startOfDay(now) - startOfDay(at)) / 86_400_000);
  if (dayDiff === 0) {
    return `Today at ${time}`;
  }
  if (dayDiff === 1) {
    return `Yesterday at ${time}`;
  }
  const date = at.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
  return `${date} at ${time}`;
}

function startOfDay(date: Date): number {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
}
