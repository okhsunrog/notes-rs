import type { Root } from "mdast";
import { describe, expect, it } from "vite-plus/test";
import {
  MAX_MATH_DOCUMENT_CHARS,
  MAX_MATH_EXPRESSION_CHARS,
  MAX_MATH_EXPRESSIONS,
  remarkMathLimits,
} from "./remark-math-limits";

describe("remarkMathLimits", () => {
  it("turns an oversized expression into recoverable inert code", () => {
    const source = "x".repeat(MAX_MATH_EXPRESSION_CHARS + 1);
    const tree = {
      type: "root",
      children: [{ type: "paragraph", children: [{ type: "inlineMath", value: source }] }],
    } as unknown as Root;

    remarkMathLimits()(tree);

    expect(tree.children[0]).toMatchObject({
      type: "paragraph",
      children: [{ type: "inlineCode", value: source }],
    });
  });

  it("bounds the total number of expressions passed to KaTeX", () => {
    const children = Array.from({ length: MAX_MATH_EXPRESSIONS + 1 }, (_, index) => ({
      type: "math",
      value: `x_${index}`,
    }));
    const tree = { type: "root", children } as unknown as Root;

    remarkMathLimits()(tree);

    expect(tree.children[MAX_MATH_EXPRESSIONS - 1]).toMatchObject({ type: "math" });
    expect(tree.children[MAX_MATH_EXPRESSIONS]).toMatchObject({
      type: "code",
      lang: "text",
      value: `x_${MAX_MATH_EXPRESSIONS}`,
    });
  });

  it("bounds aggregate KaTeX input across a document", () => {
    const expression = "x".repeat(MAX_MATH_EXPRESSION_CHARS);
    const acceptedCount = MAX_MATH_DOCUMENT_CHARS / MAX_MATH_EXPRESSION_CHARS;
    const children = Array.from({ length: acceptedCount + 1 }, () => ({
      type: "inlineMath",
      value: expression,
    }));
    const tree = {
      type: "root",
      children: [{ type: "paragraph", children }],
    } as unknown as Root;

    remarkMathLimits()(tree);

    const paragraph = tree.children[0];
    expect(paragraph).toMatchObject({ type: "paragraph" });
    if (paragraph.type !== "paragraph") throw new Error("expected paragraph");
    expect(paragraph.children[acceptedCount - 1]).toMatchObject({ type: "inlineMath" });
    expect(paragraph.children[acceptedCount]).toMatchObject({
      type: "inlineCode",
      value: expression,
    });
  });

  it("also bounds fenced math handled directly by rehype-katex", () => {
    const source = "x".repeat(MAX_MATH_EXPRESSION_CHARS + 1);
    const tree = {
      type: "root",
      children: [{ type: "code", lang: "math", value: source }],
    } as unknown as Root;

    remarkMathLimits()(tree);

    expect(tree.children[0]).toMatchObject({ type: "code", lang: "text", value: source });
  });
});
