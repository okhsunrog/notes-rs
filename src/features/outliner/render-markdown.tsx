import { Fragment, type ReactNode } from "react";

/**
 * Lightweight markdown-ish renderer for block view mode. Recognizes inline
 * code, `[[wikilinks]]`, `((block-refs))`, `**bold**`, and `*italic*`. Not a
 * full Markdown parser — we never render headings, lists, or block-level
 * constructs, because the outliner already provides block structure.
 *
 * Precedence (highest first):
 *   1. inline code  `…`        — content inside is literal
 *   2. wikilink     [[Title]]
 *   3. block-ref    ((uuid))
 *   4. bold         **text**
 *   5. italic       *text*
 *
 * Anything that doesn't match is rendered as plain text, so partial/typo
 * syntax (`**no close`) is harmless.
 */
const PATTERN = /(`[^`\n]+`)|(\[\[[^\]\n]+\]\])|(\(\([^)\n]+\)\))|(\*\*[^*\n]+\*\*)|(\*[^*\n]+\*)/g;

export function renderMarkdown(text: string): ReactNode[] {
  if (!text) return [];
  const out: ReactNode[] = [];
  let lastIndex = 0;
  let key = 0;

  // exec() in a loop so we can capture each match's index and emit the
  // plain-text gap before it.
  PATTERN.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = PATTERN.exec(text)) !== null) {
    if (m.index > lastIndex) {
      out.push(<Fragment key={key++}>{text.slice(lastIndex, m.index)}</Fragment>);
    }
    const [matched, code, wiki, ref, bold, italic] = m;
    if (code) {
      out.push(
        <code key={key++} className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]">
          {code.slice(1, -1)}
        </code>,
      );
    } else if (wiki) {
      out.push(
        <span
          key={key++}
          className="cursor-pointer rounded text-sky-600 hover:underline dark:text-sky-400"
          data-wikilink={wiki.slice(2, -2)}
        >
          {wiki.slice(2, -2)}
        </span>,
      );
    } else if (ref) {
      out.push(
        <span
          key={key++}
          className="cursor-pointer rounded bg-muted/60 px-1 font-mono text-[0.85em] text-muted-foreground hover:text-foreground"
          data-blockref={ref.slice(2, -2)}
        >
          (({ref.slice(2, -2)}))
        </span>,
      );
    } else if (bold) {
      out.push(
        <strong key={key++} className="font-semibold">
          {bold.slice(2, -2)}
        </strong>,
      );
    } else if (italic) {
      out.push(
        <em key={key++} className="italic">
          {italic.slice(1, -1)}
        </em>,
      );
    }
    lastIndex = m.index + matched.length;
  }
  if (lastIndex < text.length) {
    out.push(<Fragment key={key++}>{text.slice(lastIndex)}</Fragment>);
  }
  return out;
}
