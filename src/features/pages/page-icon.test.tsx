import { expect, it } from "vite-plus/test";
import { FileText, PenLine } from "lucide-react";
import { pageIconFor } from "./page-icon";

it("marks handwritten notes with the pen and every other page with the document", () => {
  expect(pageIconFor({ kind: "handwriting" })).toBe(PenLine);
  expect(pageIconFor({ kind: "note" })).toBe(FileText);
  expect(pageIconFor({ kind: "journal", date: "2026-09-06" })).toBe(FileText);
});
