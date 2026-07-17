import { describe, expect, it } from "vite-plus/test";
import {
  attachmentImageHref,
  classifyMarkdownImageSource,
  safeMarkdownImageSourceTransform,
  type MarkdownResolvedImage,
  validateResolvedMarkdownImage,
} from "./image-policy";

const ATTACHMENT_UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";

describe("Markdown image policy", () => {
  it("round-trips only typed attachment references", () => {
    expect(classifyMarkdownImageSource(attachmentImageHref(ATTACHMENT_UUID.toUpperCase()))).toEqual(
      {
        kind: "attachment",
        attachmentUuid: ATTACHMENT_UUID,
        href: `notes-attachment:${ATTACHMENT_UUID}`,
      },
    );
  });

  it.each([
    "javascript:alert(1)",
    "file:///etc/passwd",
    "data:image/svg+xml,<svg onload=alert(1) />",
    "notes-attachment:not-a-uuid",
    "../secret.png",
  ])("blocks unsafe or ambiguous image source %s", (source) => {
    expect(classifyMarkdownImageSource(source).kind).toBe("blocked");
    expect(safeMarkdownImageSourceTransform(source)).toBe("");
  });

  it("classifies remote images without authorizing a load", () => {
    expect(classifyMarkdownImageSource("https://tracker.example/pixel.png")).toEqual({
      kind: "remote",
      href: "https://tracker.example/pixel.png",
      hostname: "tracker.example",
    });
  });

  it("accepts bounded local image handles returned by the trusted resolver", () => {
    const image = {
      byteSize: 24_000,
      height: 480,
      mime: "image/png",
      src: "blob:https://tauri.localhost/019c8d1a-4ab1-7f31-8f00-f594337c3ca5",
      width: 640,
    } satisfies MarkdownResolvedImage;

    expect(validateResolvedMarkdownImage(image)).toEqual({ kind: "safe", image });
  });

  it("rejects remote resolver output, oversized images, and unsanitized SVG", () => {
    expect(
      validateResolvedMarkdownImage({
        byteSize: 1,
        height: 1,
        mime: "image/png",
        src: "https://tracker.example/pixel.png",
        width: 1,
      }),
    ).toEqual({ kind: "blocked", reason: "unsafe_src" });
    expect(
      validateResolvedMarkdownImage({
        byteSize: 33 * 1024 * 1024,
        height: 1,
        mime: "image/png",
        src: "blob:https://tauri.localhost/too-large",
        width: 1,
      }),
    ).toEqual({ kind: "blocked", reason: "invalid_size" });
    expect(
      validateResolvedMarkdownImage({
        byteSize: 512,
        height: 480,
        mime: "image/svg+xml",
        src: "asset://localhost/diagram.svg",
        width: 640,
      }),
    ).toEqual({ kind: "blocked", reason: "unsanitized_svg" });
  });

  it.each([
    "asset://user@localhost/diagram.png",
    "http://asset.localhost:8080/diagram.png",
    "blob:https://evil.example/remote-capability",
    "blob:https://user@tauri.localhost/credentialed-capability",
    "blob:javascript:alert(1)",
  ])("rejects a resolver URL outside the exact local capability boundary: %s", (src) => {
    expect(
      validateResolvedMarkdownImage({
        byteSize: 512,
        height: 16,
        mime: "image/png",
        src,
        width: 16,
      }),
    ).toEqual({ kind: "blocked", reason: "unsafe_src" });
  });

  it("rejects dimensionless or decompression-sized resolver metadata", () => {
    const dimensionless = {
      byteSize: 512,
      mime: "image/png",
      src: "asset://localhost/dimensionless.png",
    } as unknown as MarkdownResolvedImage;

    expect(validateResolvedMarkdownImage(dimensionless)).toEqual({
      kind: "blocked",
      reason: "invalid_size",
    });
    expect(
      validateResolvedMarkdownImage({
        byteSize: 512,
        height: 5_000,
        mime: "image/png",
        src: "asset://localhost/huge.png",
        width: 5_001,
      }),
    ).toEqual({ kind: "blocked", reason: "invalid_size" });
  });
});
