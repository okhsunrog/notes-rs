import { Fragment, type ReactNode } from "react";
import { BlockRef, WikiLink } from "./ref-preview";

/**
 * Lightweight inline Markdown renderer shared by page views. Block-level
 * structure (heading, list, quote, code, divider) comes from `BlockStyle` in
 * the caller; this function handles inline code, links, references and basic
 * emphasis inside that durable block shape.
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
      out.push(<WikiLink key={key++} title={wiki.slice(2, -2)} />);
    } else if (ref) {
      out.push(<BlockRef key={key++} uuid={ref.slice(2, -2)} />);
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
