/**
 * The name a handwritten note is born with.
 *
 * A sheet cannot show a caret, so a new note has to arrive already named: the alternative is a
 * text field on the drawing screen, which is what put a soft keyboard under an armed pen in the
 * first place. Every reference app does the same — a timestamp or "Document <date>" — and the
 * name stays editable from the rename dialog.
 *
 * Local time, fixed digits: sorted lexically it is also sorted chronologically, and it never
 * depends on the device locale the way `toLocaleString` would.
 */
export function defaultHandwritingTitle(now: Date = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  const date = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
  return `Handwriting ${date} ${pad(now.getHours())}:${pad(now.getMinutes())}`;
}
