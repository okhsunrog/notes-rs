import { describe, expect, it } from "vite-plus/test";
import { attachmentResourceUrl } from "./use-attachment-images";

const descriptor = {
  attachmentUuid: "019c8d1a-4ab1-7f31-8f00-f594337c3ca5",
  blobHash: "a".repeat(64),
  resourceVersion: 3,
};

describe("attachmentResourceUrl", () => {
  it("keeps typed route separators outside convertFileSrc on desktop", () => {
    expect(attachmentResourceUrl("notes-attachment://localhost/", descriptor, "preview")).toBe(
      `notes-attachment://localhost/v3/${descriptor.attachmentUuid}/${descriptor.blobHash}/preview`,
    );
  });

  it("uses the Android custom-protocol origin for the same route", () => {
    expect(attachmentResourceUrl("http://notes-attachment.localhost", descriptor, "original")).toBe(
      `http://notes-attachment.localhost/v3/${descriptor.attachmentUuid}/${descriptor.blobHash}/original`,
    );
  });
});
