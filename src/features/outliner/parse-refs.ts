/** Pull `[[wikilinks]]` and `((block-refs))` out of a block's raw markdown.
 *
 * Matches the same shapes as `render-markdown.tsx`. Inline code is *not*
 * stripped first — we accept that a wikilink inside backticks (`` `[[x]]` ``)
 * still creates an edge. Worth fixing if it ever becomes annoying. */
export function parseRefs(text: string): { wikilinks: string[]; blockRefs: string[] } {
  if (!text) return { wikilinks: [], blockRefs: [] };
  const wiki = new Set<string>();
  const refs = new Set<string>();
  const WIKI = /\[\[([^\]\n]+)\]\]/g;
  const REF = /\(\(([^)\n]+)\)\)/g;
  for (const m of text.matchAll(WIKI)) wiki.add(m[1].trim());
  for (const m of text.matchAll(REF)) refs.add(m[1].trim());
  return {
    wikilinks: [...wiki].filter((s) => s.length > 0),
    blockRefs: [...refs].filter((s) => s.length > 0),
  };
}
