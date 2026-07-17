import type { Root } from "mdast";

export const MAX_MATH_EXPRESSION_CHARS = 8_192;
export const MAX_MATH_EXPRESSIONS = 256;
export const MAX_MATH_DOCUMENT_CHARS = 65_536;

/**
 * Prevents untrusted Markdown from driving unbounded synchronous KaTeX work.
 * Oversized/excess expressions stay visible as inert code and the durable
 * Markdown source remains untouched.
 */
export function remarkMathLimits() {
  return (tree: Root): void => {
    let expressionCount = 0;
    let documentMathChars = 0;
    visit(tree as MutableMdastNode, (node) => {
      const isMathNode = node.type === "math" || node.type === "inlineMath";
      const isMathFence = node.type === "code" && node.lang === "math";
      if (!isMathNode && !isMathFence) return;
      expressionCount += 1;
      documentMathChars += node.value?.length ?? 0;
      if (
        expressionCount <= MAX_MATH_EXPRESSIONS &&
        (node.value?.length ?? 0) <= MAX_MATH_EXPRESSION_CHARS &&
        documentMathChars <= MAX_MATH_DOCUMENT_CHARS
      ) {
        return;
      }

      if (isMathFence) {
        node.lang = "text";
        return;
      }
      const wasBlock = node.type === "math";
      node.type = wasBlock ? "code" : "inlineCode";
      if (wasBlock) node.lang = "text";
      delete node.data;
      delete node.meta;
    });
  };
}

interface MutableMdastNode {
  children?: MutableMdastNode[];
  data?: unknown;
  lang?: string | null;
  meta?: string | null;
  type: string;
  value?: string;
}

function visit(node: MutableMdastNode, visitor: (node: MutableMdastNode) => void): void {
  visitor(node);
  for (const child of node.children ?? []) visit(child, visitor);
}
