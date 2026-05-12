/** Autocomplete trigger detected in a block's edit buffer. */
export type Trigger = {
  kind: "[[" | "((";
  /** Index in the text where the trigger opens (the `[` or `(`). */
  start: number;
  /** Caret index (text past `start` up to here is the live query). */
  end: number;
  query: string;
};

/** Scan `text[0..caret]` and return the rightmost unclosed `[[` or `((`. The
 * trigger is "unclosed" if no `]]` / `))` appears between it and the caret.
 * Whitespace and newlines inside the query break the trigger (Logseq-style). */
export function detectTrigger(text: string, caret: number): Trigger | null {
  const slice = text.slice(0, caret);
  let best: Trigger | null = null;

  for (const kind of ["[[", "(("] as const) {
    const closer = kind === "[[" ? "]]" : "))";
    let from = 0;
    while (true) {
      const i = slice.indexOf(kind, from);
      if (i < 0) break;
      // Walk forward from after `kind` to caret; reject if we see closer, newline, or another opener.
      const queryStart = i + 2;
      const inside = slice.slice(queryStart);
      if (inside.includes(closer)) {
        from = i + 2;
        continue;
      }
      if (inside.includes("\n")) {
        from = i + 2;
        continue;
      }
      const candidate: Trigger = {
        kind,
        start: i,
        end: caret,
        query: inside,
      };
      if (!best || candidate.start > best.start) best = candidate;
      from = i + 2;
    }
  }
  return best;
}
