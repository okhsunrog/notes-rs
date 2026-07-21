import { describe, expect, it } from "vite-plus/test";
import {
  attachmentImageHref,
  classifyMarkdownImageSource,
  extractMarkdownAttachmentUuids,
  safeMarkdownImageSourceTransform,
  type MarkdownResolvedImage,
  validateResolvedMarkdownImage,
} from "./image-policy";

const ATTACHMENT_UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca5";
const OTHER_ATTACHMENT_UUID = "019c8d1a-4ab1-7f31-8f00-f594337c3ca6";
const BLOB_HASH = "a".repeat(64);
const DESKTOP_URL = `notes-attachment://localhost/v1/${ATTACHMENT_UUID}/${BLOB_HASH}/preview`;
const DESKTOP_ORIGINAL_URL = `notes-attachment://localhost/v1/${ATTACHMENT_UUID}/${BLOB_HASH}/original`;
const ANDROID_URL = `http://notes-attachment.localhost/v1/${ATTACHMENT_UUID}/${BLOB_HASH}/preview`;
const ANDROID_ORIGINAL_URL = `http://notes-attachment.localhost/v1/${ATTACHMENT_UUID}/${BLOB_HASH}/original`;

function resolvedImage(overrides: Partial<MarkdownResolvedImage> = {}): MarkdownResolvedImage {
  return {
    byteSize: 24_000,
    height: 480,
    mime: "image/png",
    originalSrc: DESKTOP_ORIGINAL_URL,
    src: DESKTOP_URL,
    width: 640,
    ...overrides,
  };
}

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

  it("extracts, canonicalizes, and deduplicates attachment references", () => {
    expect(
      extractMarkdownAttachmentUuids(
        `![one](notes-attachment:${ATTACHMENT_UUID.toUpperCase()})\n![again](notes-attachment:${ATTACHMENT_UUID})\n![two](notes-attachment:${OTHER_ATTACHMENT_UUID})`,
      ),
    ).toEqual([ATTACHMENT_UUID, OTHER_ATTACHMENT_UUID]);
  });

  it.each([
    [DESKTOP_URL, DESKTOP_ORIGINAL_URL],
    [ANDROID_URL, ANDROID_ORIGINAL_URL],
  ])(
    "accepts the exact platform-local image route returned by the trusted resolver: %s",
    (src, originalSrc) => {
      const image = resolvedImage({ src, originalSrc });
      expect(validateResolvedMarkdownImage(image, ATTACHMENT_UUID)).toEqual({
        kind: "safe",
        image,
      });
    },
  );

  it("binds the local capability URL to the requested attachment UUID", () => {
    expect(validateResolvedMarkdownImage(resolvedImage(), OTHER_ATTACHMENT_UUID)).toEqual({
      kind: "blocked",
      reason: "unsafe_src",
    });
  });

  it("requires preview and original routes to name the same immutable resource", () => {
    expect(
      validateResolvedMarkdownImage(
        resolvedImage({ originalSrc: DESKTOP_ORIGINAL_URL.replace(BLOB_HASH, "b".repeat(64)) }),
        ATTACHMENT_UUID,
      ),
    ).toEqual({ kind: "blocked", reason: "unsafe_src" });
    expect(
      validateResolvedMarkdownImage(
        resolvedImage({ originalSrc: DESKTOP_ORIGINAL_URL.replace("/v1/", "/v2/") }),
        ATTACHMENT_UUID,
      ),
    ).toEqual({ kind: "blocked", reason: "unsafe_src" });
  });

  it("rejects remote resolver output, oversized images, and unsupported formats", () => {
    expect(
      validateResolvedMarkdownImage(
        resolvedImage({
          byteSize: 1,
          height: 1,
          src: "https://tracker.example/pixel.png",
          width: 1,
        }),
        ATTACHMENT_UUID,
      ),
    ).toEqual({ kind: "blocked", reason: "unsafe_src" });
    expect(
      validateResolvedMarkdownImage(
        resolvedImage({
          byteSize: 33 * 1024 * 1024,
          height: 1,
          width: 1,
        }),
        ATTACHMENT_UUID,
      ),
    ).toEqual({ kind: "blocked", reason: "invalid_size" });
    expect(
      validateResolvedMarkdownImage(
        {
          ...resolvedImage(),
          mime: "image/svg+xml",
        } as unknown as MarkdownResolvedImage,
        ATTACHMENT_UUID,
      ),
    ).toEqual({ kind: "blocked", reason: "unsafe_mime" });
  });

  it.each([
    `notes-attachment://user@localhost/${ATTACHMENT_UUID}`,
    `notes-attachment://localhost:8080/${ATTACHMENT_UUID}`,
    `notes-attachment://localhost/${OTHER_ATTACHMENT_UUID}`,
    `http://notes-attachment.localhost/${ATTACHMENT_UUID}?token=leak`,
    `asset://localhost/${ATTACHMENT_UUID}`,
    "blob:https://tauri.localhost/local-capability",
  ])("rejects a resolver URL outside the exact local capability boundary: %s", (src) => {
    expect(validateResolvedMarkdownImage(resolvedImage({ src }), ATTACHMENT_UUID)).toEqual({
      kind: "blocked",
      reason: "unsafe_src",
    });
  });

  it("rejects dimensionless or decompression-sized resolver metadata", () => {
    const dimensionless = {
      byteSize: 512,
      mime: "image/png",
      src: DESKTOP_URL,
    } as unknown as MarkdownResolvedImage;

    expect(validateResolvedMarkdownImage(dimensionless, ATTACHMENT_UUID)).toEqual({
      kind: "blocked",
      reason: "invalid_size",
    });
    expect(
      validateResolvedMarkdownImage(
        resolvedImage({
          byteSize: 512,
          height: 5_000,
          width: 5_001,
        }),
        ATTACHMENT_UUID,
      ),
    ).toEqual({ kind: "blocked", reason: "invalid_size" });
  });
});
