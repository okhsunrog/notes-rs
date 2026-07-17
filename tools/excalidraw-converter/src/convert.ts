import fs from "node:fs/promises";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { createRequire } from "node:module";

import { ExcalidrawBrowser } from "./browser.ts";
import type { ConverterOptions } from "./args.ts";
import type {
  ConversionBundle,
  DrawingFailureCode,
  DrawingResult,
  SourceMetadata,
} from "./schema.ts";
import { createPublication, installPng, readSourceDrawing, validateRoots } from "./io.ts";

const require = createRequire(import.meta.url);
const TOOL_PACKAGE = require("../package.json");
const EXCALIDRAW_PACKAGE = require("@excalidraw/excalidraw/package.json");
const PLAYWRIGHT_PACKAGE = require("playwright/package.json");

const ERROR_MESSAGES = Object.freeze({
  external_asset_blocked: "drawing references a network asset, which is not permitted",
  invalid_excalidraw_json: "source is not valid JSON",
  output_encode_failed: "rendered output is not a valid bounded PNG",
  render_failed: "Excalidraw PNG render failed",
  resource_limit_exceeded: "source exceeds the converter size limit",
  unsupported_excalidraw_schema: "source is not a supported Excalidraw version-2 document",
} satisfies Record<DrawingFailureCode, string>);

function failed(source: SourceMetadata, code: DrawingFailureCode): DrawingResult {
  return {
    source,
    status: "failed",
    error: { code, message: ERROR_MESSAGES[code] },
  };
}

function hasExternalFiles(files: unknown): boolean {
  if (!files || typeof files !== "object" || Array.isArray(files)) {
    return false;
  }
  return Object.values(files).some((file) => {
    if (!file || typeof file !== "object") {
      return false;
    }
    const record = file as Record<string, unknown>;
    return typeof record.dataURL === "string" && !record.dataURL.startsWith("data:");
  });
}

function validateDocument(document: unknown): document is Record<string, unknown> {
  if (document === null || typeof document !== "object" || Array.isArray(document)) {
    return false;
  }
  const record = document as Record<string, unknown>;
  return (
    record.type === "excalidraw" &&
    record.version === 2 &&
    Array.isArray(record.elements) &&
    (record.files === undefined ||
      (record.files !== null && typeof record.files === "object" && !Array.isArray(record.files)))
  );
}

export async function convertDrawings(options: ConverterOptions): Promise<ConversionBundle> {
  const roots = await validateRoots(options.sourceRoot, options.outputRoot);
  const publication = await createPublication(roots.outputRoot);
  let browser: ExcalidrawBrowser | null = null;

  try {
    browser = await ExcalidrawBrowser.launch();
    const drawings: DrawingResult[] = [];
    const downloads = path.join(publication.stagingRoot, ".downloads");
    await fs.mkdir(downloads, { mode: 0o700 });

    for (const relativePath of options.drawings) {
      const input = await readSourceDrawing(roots.sourceRoot, relativePath);
      const source = {
        relativePath,
        sizeBytes: input.sizeBytes,
        sha256: input.sha256,
      };
      if (input.tooLarge) {
        drawings.push(failed(source, "resource_limit_exceeded"));
        continue;
      }

      let document: unknown;
      try {
        document = JSON.parse(input.bytes.toString("utf8"));
      } catch {
        drawings.push(failed(source, "invalid_excalidraw_json"));
        continue;
      }
      if (!validateDocument(document)) {
        drawings.push(failed(source, "unsupported_excalidraw_schema"));
        continue;
      }
      if (hasExternalFiles(document.files)) {
        drawings.push(failed(source, "external_asset_blocked"));
        continue;
      }

      const temporaryPath = path.join(downloads, `${randomUUID()}.png`);
      try {
        const result = await browser.render(document, temporaryPath);
        if (result.status === "skipped_empty") {
          drawings.push({ source, status: "skipped_empty" });
        } else if (result.status === "failed" && result.blockedNetwork) {
          drawings.push(failed(source, "external_asset_blocked"));
        } else {
          try {
            const output = await installPng(publication.stagingRoot, temporaryPath, relativePath);
            drawings.push({ source, status: "converted", output });
          } catch {
            await fs.rm(temporaryPath, { force: true });
            drawings.push(failed(source, "output_encode_failed"));
          }
        }
      } catch {
        await fs.rm(temporaryPath, { force: true });
        drawings.push(failed(source, "render_failed"));
      }
    }

    await fs.rm(downloads, { recursive: true, force: true });
    const bundle: ConversionBundle = {
      schemaVersion: 1,
      converter: {
        name: TOOL_PACKAGE.name,
        version: TOOL_PACKAGE.version,
        excalidrawVersion: EXCALIDRAW_PACKAGE.version,
        playwrightVersion: PLAYWRIGHT_PACKAGE.version,
        browserName: "chromium",
        browserVersion: browser.browserVersion,
      },
      sourceRoot: {
        kind: "redacted",
        manifestSha256: options.sourceManifestSha256,
      },
      drawings,
    };
    await publication.publish(bundle);
    return bundle;
  } finally {
    await browser?.close();
    await publication.discard();
  }
}
