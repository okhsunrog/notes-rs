import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import http from "node:http";
import type { AddressInfo } from "node:net";
import os from "node:os";
import path from "node:path";
import test, { type TestContext } from "node:test";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";

import { boundedDimensions, ExcalidrawBrowser } from "../src/browser.ts";
import { convertDrawings } from "../src/convert.ts";
import type { ConverterOptions } from "../src/args.ts";
import type { DrawingResult } from "../src/schema.ts";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURES = path.join(HERE, "fixtures");
const MANIFEST_SHA = "1".repeat(64);

async function workspace(t: TestContext) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "tangleaf-excalidraw-test-"));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const source = path.join(root, "source");
  await fs.cp(FIXTURES, path.join(source, "draws"), { recursive: true });
  return { root, source };
}

async function treeDigest(root: string): Promise<string> {
  const hash = createHash("sha256");
  async function visit(directory: string, prefix = "") {
    const entries = await fs.readdir(directory, { withFileTypes: true });
    entries.sort((a, b) => a.name.localeCompare(b.name, "en"));
    for (const entry of entries) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      hash.update(`${entry.isDirectory() ? "d" : "f"}:${relative}\0`);
      const absolute = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        await visit(absolute, relative);
      } else {
        hash.update(await fs.readFile(absolute));
      }
    }
  }
  await visit(root);
  return hash.digest("hex");
}

function options(source: string, output: string, drawings: string[]): ConverterOptions {
  return {
    sourceRoot: source,
    outputRoot: output,
    sourceManifestSha256: MANIFEST_SHA,
    drawings,
  };
}

function requiredDrawing(
  drawings: Map<string, DrawingResult>,
  relativePath: string,
): DrawingResult {
  const drawing = drawings.get(relativePath);
  assert.ok(drawing, `missing bundle entry for ${relativePath}`);
  return drawing;
}

function firstRgbaPixel(png: Buffer): [number, number, number, number] {
  assert.equal(png.readUInt8(24), 8, "fixture PNG must use 8-bit channels");
  assert.equal(png.readUInt8(25), 6, "fixture PNG must be RGBA");
  const chunks: Buffer[] = [];
  for (let offset = 8; offset < png.byteLength; ) {
    const length = png.readUInt32BE(offset);
    const kind = png.toString("ascii", offset + 4, offset + 8);
    if (kind === "IDAT") {
      chunks.push(png.subarray(offset + 8, offset + 8 + length));
    }
    offset += 12 + length;
  }
  const scanlines = inflateSync(Buffer.concat(chunks));
  assert.ok(scanlines[0]! <= 4, "unsupported PNG row filter");
  return [scanlines[1]!, scanlines[2]!, scanlines[3]!, scanlines[4]!];
}

void test("dimension policy uses 2x output and enforces axis and pixel budgets", () => {
  assert.deepEqual(boundedDimensions(100, 50), { width: 200, height: 100, scale: 2 });
  const square = boundedDimensions(10_000, 10_000);
  assert.deepEqual(square, { width: 5000, height: 5000, scale: 0.5 });
  const wide = boundedDimensions(20_000, 1000);
  assert.equal(wide.width, 8192);
  assert.ok(wide.height <= 8192);
  assert.ok(wide.width * wide.height <= 25_000_000);
});

void test("official export converts freehand, text, and embedded files and reports empty/invalid", async (t) => {
  const { root, source } = await workspace(t);
  const output = path.join(root, "publication");
  const before = await treeDigest(source);
  const drawings = [
    "draws/empty.excalidraw",
    "draws/files.excalidraw",
    "draws/freedraw.excalidraw",
    "draws/invalid.excalidraw",
    "draws/text.excalidraw",
  ];

  const bundle = await convertDrawings(options(source, output, drawings));
  assert.equal(
    await treeDigest(source),
    before,
    "source graph bytes and paths must remain unchanged",
  );
  assert.equal(bundle.schemaVersion, 1);
  assert.deepEqual(bundle.sourceRoot, { kind: "redacted", manifestSha256: MANIFEST_SHA });
  assert.deepEqual(bundle.converter, {
    name: "@tangleaf/excalidraw-converter",
    version: "1.0.0",
    excalidrawVersion: "0.12.0",
    playwrightVersion: "1.61.1",
    browserName: "chromium",
    browserVersion: bundle.converter.browserVersion,
  });

  const byPath = new Map(bundle.drawings.map((drawing) => [drawing.source.relativePath, drawing]));
  assert.equal(requiredDrawing(byPath, "draws/empty.excalidraw").status, "skipped_empty");
  const invalid = requiredDrawing(byPath, "draws/invalid.excalidraw");
  assert.equal(invalid.status, "failed");
  if (invalid.status !== "failed") {
    throw new Error("invalid fixture unexpectedly converted");
  }
  assert.deepEqual(invalid.error, {
    code: "invalid_excalidraw_json",
    message: "source is not valid JSON",
  });
  for (const relativePath of [
    "draws/files.excalidraw",
    "draws/freedraw.excalidraw",
    "draws/text.excalidraw",
  ]) {
    const drawing = requiredDrawing(byPath, relativePath);
    assert.equal(drawing.status, "converted", relativePath);
    if (drawing.status !== "converted") {
      throw new Error(`${relativePath} unexpectedly failed conversion`);
    }
    assert.equal(drawing.output.mimeType, "image/png");
    assert.ok(drawing.output.width > 0 && drawing.output.width <= 8192);
    assert.ok(drawing.output.height > 0 && drawing.output.height <= 8192);
    assert.ok(drawing.output.width * drawing.output.height <= 25_000_000);
    const bytes = await fs.readFile(path.join(output, drawing.output.relativePath));
    assert.equal(bytes.byteLength, drawing.output.sizeBytes);
    assert.equal(createHash("sha256").update(bytes).digest("hex"), drawing.output.sha256);
    if (relativePath === "draws/freedraw.excalidraw") {
      assert.deepEqual(firstRgbaPixel(bytes), [248, 249, 250, 255]);
    }
    if (relativePath === "draws/text.excalidraw") {
      assert.deepEqual(firstRgbaPixel(bytes), [255, 255, 255, 255]);
    }
  }

  const bundleText = await fs.readFile(path.join(output, "conversion-bundle.json"), "utf8");
  assert.deepEqual(JSON.parse(bundleText), bundle);
  assert.ok(!bundleText.includes(source));
  assert.ok(!bundleText.includes("Synthetic fixture"), "drawing text must not leak into metadata");
});

void test("network-backed files are rejected without reaching the network", async (t) => {
  const { root, source } = await workspace(t);
  let requests = 0;
  const server = http.createServer((_request, response) => {
    requests += 1;
    response.writeHead(200, { "content-type": "image/png" });
    response.end();
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", () => resolve()));
  t.after(() => new Promise<void>((resolve) => server.close(() => resolve())));
  const { port } = server.address() as AddressInfo;

  const browser = await ExcalidrawBrowser.launch();
  t.after(() => browser.close());
  const networkAllowed = await browser.page.evaluate(async (url) => {
    try {
      await fetch(url);
      return true;
    } catch {
      return false;
    }
  }, `http://127.0.0.1:${port}/browser-probe`);
  assert.equal(networkAllowed, false, "browser CSP/route policy must deny network fetches");
  assert.equal(requests, 0);

  const networkDocument = JSON.parse(
    await fs.readFile(path.join(source, "draws/files.excalidraw"), "utf8"),
  );
  networkDocument.files.file000000000000000001.dataURL = `http://127.0.0.1:${port}/pixel.png`;
  await fs.writeFile(
    path.join(source, "draws/network.excalidraw"),
    `${JSON.stringify(networkDocument)}\n`,
  );

  const bundle = await convertDrawings(
    options(source, path.join(root, "network-publication"), ["draws/network.excalidraw"]),
  );
  const [drawing] = bundle.drawings;
  assert.ok(drawing);
  assert.equal(drawing.status, "failed");
  if (drawing.status !== "failed") {
    throw new Error("network fixture unexpectedly converted");
  }
  assert.equal(drawing.error.code, "external_asset_blocked");
  assert.equal(requests, 0);
});

void test("pinned browser output and metadata are deterministic", async (t) => {
  const { root, source } = await workspace(t);
  const selected = ["draws/freedraw.excalidraw", "draws/text.excalidraw"];
  const first = path.join(root, "first");
  const second = path.join(root, "second");
  const firstBundle = await convertDrawings(options(source, first, selected));
  const secondBundle = await convertDrawings(options(source, second, selected));

  assert.deepEqual(secondBundle, firstBundle);
  assert.equal(
    await fs.readFile(path.join(first, "conversion-bundle.json"), "utf8"),
    await fs.readFile(path.join(second, "conversion-bundle.json"), "utf8"),
  );
  for (const drawing of firstBundle.drawings) {
    assert.equal(drawing.status, "converted");
    if (drawing.status !== "converted") {
      throw new Error("determinism fixture unexpectedly failed conversion");
    }
    assert.deepEqual(
      await fs.readFile(path.join(first, drawing.output.relativePath)),
      await fs.readFile(path.join(second, drawing.output.relativePath)),
    );
  }
});

void test("publication cannot overlap source or replace an existing output", async (t) => {
  const { root, source } = await workspace(t);
  const selected = ["draws/freedraw.excalidraw"];
  await assert.rejects(
    convertDrawings(options(source, path.join(source, "generated"), selected)),
    /disjoint/,
  );

  const existing = path.join(root, "existing");
  await fs.mkdir(existing);
  await fs.writeFile(path.join(existing, "sentinel"), "keep");
  await assert.rejects(convertDrawings(options(source, existing, selected)), /already exists/);
  assert.equal(await fs.readFile(path.join(existing, "sentinel"), "utf8"), "keep");
});

void test("a symlinked drawing cannot escape the source root", async (t) => {
  const { root, source } = await workspace(t);
  const outside = path.join(root, "outside.excalidraw");
  await fs.copyFile(path.join(source, "draws/freedraw.excalidraw"), outside);
  await fs.symlink(outside, path.join(source, "draws/escape.excalidraw"));
  await assert.rejects(
    convertDrawings(
      options(source, path.join(root, "escape-publication"), ["draws/escape.excalidraw"]),
    ),
    /outside the source root/,
  );
  await assert.rejects(fs.stat(path.join(root, "escape-publication")), { code: "ENOENT" });
});
