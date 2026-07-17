import { describe, expect, it } from "vite-plus/test";
import {
  blockTargetHref,
  classifyMarkdownUrl,
  pageTargetHref,
  safeMarkdownUrlTransform,
} from "./url-policy";

const BLOCK_UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";

describe("Markdown URL policy", () => {
  it("round-trips typed page and block targets", () => {
    expect(classifyMarkdownUrl(pageTargetHref("Проект / Aurora"))).toEqual({
      kind: "allowed",
      href: "notes-page:%D0%9F%D1%80%D0%BE%D0%B5%D0%BA%D1%82%20%2F%20Aurora",
      target: { kind: "page", title: "Проект / Aurora" },
    });
    expect(classifyMarkdownUrl(blockTargetHref(BLOCK_UUID.toUpperCase()))).toEqual({
      kind: "allowed",
      href: `notes-block:${BLOCK_UUID}`,
      target: { kind: "block", uuid: BLOCK_UUID },
    });
  });

  it("allows only explicit external protocols and fragments", () => {
    expect(classifyMarkdownUrl("https://example.com/a?b=1")).toMatchObject({
      kind: "allowed",
      target: { kind: "external", protocol: "https" },
    });
    expect(classifyMarkdownUrl("mailto:notes@example.com")).toMatchObject({
      kind: "allowed",
      target: { kind: "external", protocol: "mailto" },
    });
    expect(classifyMarkdownUrl("#architecture")).toEqual({
      kind: "allowed",
      href: "#architecture",
      target: { kind: "fragment", fragment: "architecture" },
    });
  });

  it.each(["javascript:alert(1)", "file:///etc/passwd", "data:text/html,boom", "relative.md"])(
    "blocks unsafe or ambiguous URL %s",
    (url) => {
      expect(classifyMarkdownUrl(url).kind).toBe("blocked");
      expect(safeMarkdownUrlTransform(url)).toBe("");
    },
  );

  it("rejects malformed internal targets", () => {
    expect(classifyMarkdownUrl("notes-page:%E0%A4%A")).toEqual({
      kind: "blocked",
      reason: "invalid_internal_target",
    });
    expect(classifyMarkdownUrl("notes-block:not-a-uuid")).toEqual({
      kind: "blocked",
      reason: "invalid_internal_target",
    });
    expect(classifyMarkdownUrl("notes-page:line%0Abreak")).toEqual({
      kind: "blocked",
      reason: "invalid_internal_target",
    });
  });

  it("rejects encoded control characters in mail addresses", () => {
    expect(
      classifyMarkdownUrl("mailto:notes@example.com?subject=ok%0d%0aBcc:evil@example.com"),
    ).toEqual({
      kind: "blocked",
      reason: "control_character",
    });
  });
});
