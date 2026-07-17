import assert from "node:assert/strict";
import test from "node:test";

import { normalizeRelativePath, parseArguments } from "../src/args.ts";

void test("relative drawing paths are canonical, normalized, unique, and sorted", () => {
  const options = parseArguments([
    "--source-root",
    "/tmp/source",
    "--output",
    "/tmp/output",
    "--source-manifest-sha256",
    "a".repeat(64),
    "--drawing",
    "draws/z.excalidraw",
    "--drawing",
    "draws/a.excalidraw",
    "--drawing",
    "draws/z.excalidraw",
  ]);
  assert.ok("drawings" in options);
  assert.deepEqual(options.drawings, ["draws/a.excalidraw", "draws/z.excalidraw"]);
});

void test("unsafe or ambiguous drawing paths are rejected", () => {
  for (const value of [
    "../escape.excalidraw",
    "/absolute.excalidraw",
    "draws//empty.excalidraw",
    "draws\\windows.excalidraw",
    "draws/not-json.png",
    "pages/not-in-draws.excalidraw",
    `draws/e\u0301.excalidraw`,
  ]) {
    assert.throws(() => normalizeRelativePath(value));
  }
});
