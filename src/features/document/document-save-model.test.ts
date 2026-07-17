import { describe, expect, it } from "vite-plus/test";
import { encodeDocument } from "./document-codec";
import { documentUnitsForSave } from "./document-save-model";

describe("documentUnitsForSave", () => {
  it("turns an edited continuous buffer into typed replacement units", () => {
    const base = encodeDocument([
      {
        uuid: "heading",
        parentUuid: null,
        style: { kind: "heading_1" },
        markdown: "Heading",
      },
      {
        uuid: "body",
        parentUuid: null,
        style: { kind: "paragraph" },
        markdown: "Body",
      },
    ]);

    const units = documentUnitsForSave(
      "# Heading changed\n\nBody\n\n- [x] New task",
      base.sourceMap,
    );

    expect(units).toEqual([
      {
        previousUuid: "heading",
        parentIndex: null,
        style: { kind: "heading_1" },
        markdown: "Heading changed",
      },
      {
        previousUuid: "body",
        parentIndex: null,
        style: { kind: "paragraph" },
        markdown: "Body",
      },
      {
        previousUuid: null,
        parentIndex: null,
        style: { kind: "task", state: "done" },
        markdown: "New task",
      },
    ]);
  });
});
