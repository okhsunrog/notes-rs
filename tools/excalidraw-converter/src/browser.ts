import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright";
import type { Browser, BrowserContext, Page } from "playwright";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(HERE, "..");

const SCRIPT_PATHS = [
  path.join(ROOT, "node_modules/react/umd/react.production.min.js"),
  path.join(ROOT, "node_modules/react-dom/umd/react-dom.production.min.js"),
  path.join(ROOT, "node_modules/@excalidraw/excalidraw/dist/excalidraw.production.min.js"),
];

const FONT_PATHS = [
  [
    "Virgil",
    path.join(ROOT, "node_modules/@excalidraw/excalidraw/dist/excalidraw-assets/Virgil.woff2"),
  ],
  [
    "Cascadia",
    path.join(ROOT, "node_modules/@excalidraw/excalidraw/dist/excalidraw-assets/Cascadia.woff2"),
  ],
] as const;

const LOCAL_ASSET_ORIGIN = "https://assets.tangleaf.invalid";
const LOCAL_ASSETS = new Map<string, string>([
  ["Virgil.woff2", FONT_PATHS[0][1]],
  ["Cascadia.woff2", FONT_PATHS[1][1]],
  [
    "vendor-52b1f3361986b6c6a4fe.js",
    path.join(
      ROOT,
      "node_modules/@excalidraw/excalidraw/dist/excalidraw-assets/vendor-52b1f3361986b6c6a4fe.js",
    ),
  ],
]);

type BrowserRenderResult =
  | { status: "converted" }
  | { status: "skipped_empty" }
  | { status: "failed"; blockedNetwork: true };

interface ConverterWindow extends Window {
  EXCALIDRAW_ASSET_PATH?: string;
  __tangleafDrawingDownload?: () => void;
  __tangleafDrawingDispose?: () => void;
  __tangleafBoundedDimensions?: typeof boundedDimensions;
}

export function boundedDimensions(
  width: number,
  height: number,
): { width: number; height: number; scale: number } {
  const safeWidth = Number.isFinite(width) && width > 0 ? width : 1;
  const safeHeight = Number.isFinite(height) && height > 0 ? height : 1;
  const scale = Math.min(
    2,
    8192 / safeWidth,
    8192 / safeHeight,
    Math.sqrt(25_000_000 / (safeWidth * safeHeight)),
  );
  return {
    width: Math.max(1, Math.floor(safeWidth * scale)),
    height: Math.max(1, Math.floor(safeHeight * scale)),
    scale,
  };
}

export class ExcalidrawBrowser {
  readonly browser: Browser;
  readonly context: BrowserContext;
  readonly page: Page;
  readonly browserVersion: string;
  readonly blockedRequests: string[];

  constructor(browser: Browser, context: BrowserContext, page: Page, browserVersion: string) {
    this.browser = browser;
    this.context = context;
    this.page = page;
    this.browserVersion = browserVersion;
    this.blockedRequests = [];
  }

  static async launch(): Promise<ExcalidrawBrowser> {
    const browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({
      acceptDownloads: true,
      javaScriptEnabled: true,
      serviceWorkers: "block",
      viewport: { width: 1280, height: 720 },
    });
    const page = await context.newPage();
    const instance = new ExcalidrawBrowser(browser, context, page, browser.version());

    await context.route("**/*", async (route) => {
      const url = route.request().url();
      const parsed = new URL(url);
      const localAsset =
        parsed.origin === LOCAL_ASSET_ORIGIN
          ? LOCAL_ASSETS.get(path.posix.basename(parsed.pathname))
          : undefined;
      if (localAsset) {
        await route.fulfill({ path: localAsset });
        return;
      }
      if (url.startsWith("about:") || url.startsWith("data:") || url.startsWith("blob:")) {
        await route.continue();
        return;
      }
      instance.blockedRequests.push(url);
      await route.abort("blockedbyclient");
    });

    await page.setContent(`<!doctype html>
      <html><head>
        <meta charset="utf-8">
        <meta http-equiv="Content-Security-Policy"
          content="default-src 'none'; script-src 'unsafe-inline' ${LOCAL_ASSET_ORIGIN}; style-src 'unsafe-inline'; img-src data: blob:; font-src data: ${LOCAL_ASSET_ORIGIN}; connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'">
      </head><body></body></html>`);

    await page.evaluate((assetOrigin) => {
      (window as ConverterWindow).EXCALIDRAW_ASSET_PATH = `${assetOrigin}/excalidraw`;
    }, LOCAL_ASSET_ORIGIN);
    await page.addScriptTag({
      content: `window.__tangleafBoundedDimensions = ${boundedDimensions.toString()};`,
    });

    const fontRules: string[] = [];
    for (const [family, fontPath] of FONT_PATHS) {
      const bytes = await fs.readFile(fontPath);
      fontRules.push(
        `@font-face{font-family:'${family}';src:url(data:font/woff2;base64,${bytes.toString("base64")}) format('woff2');font-style:normal;font-weight:400;font-display:block}`,
      );
    }
    await page.addStyleTag({ content: fontRules.join("\n") });
    for (const scriptPath of SCRIPT_PATHS) {
      await page.addScriptTag({ path: scriptPath });
    }
    await page.evaluate(async () => {
      await document.fonts.ready;
      const expected = ["restore", "getNonDeletedElements", "exportToBlob"];
      const library = (window as unknown as { ExcalidrawLib?: Record<string, unknown> })
        .ExcalidrawLib;
      if (!library || expected.some((name) => typeof library[name] !== "function")) {
        throw new Error("pinned Excalidraw export API is unavailable");
      }
    });
    instance.blockedRequests.length = 0;
    return instance;
  }

  async render(
    sourceDocument: Record<string, unknown>,
    destination: string,
  ): Promise<BrowserRenderResult> {
    this.blockedRequests.length = 0;
    const prepared = await this.page.evaluate<
      { status: "converted" | "skipped_empty" },
      Record<string, unknown>
    >(async (source) => {
      const { restore, getNonDeletedElements, exportToBlob } = (
        window as unknown as {
          ExcalidrawLib: {
            restore: (
              source: unknown,
              appState: null,
              elements: null,
            ) => {
              elements?: unknown[];
              appState?: Record<string, unknown>;
              files?: Record<string, unknown>;
            };
            getNonDeletedElements: (elements: unknown[]) => unknown[];
            exportToBlob: (options: Record<string, unknown>) => Promise<Blob>;
          };
        }
      ).ExcalidrawLib;
      const restored = restore(source, null, null);
      const elements = getNonDeletedElements(restored.elements ?? []);
      if (elements.length === 0) {
        return { status: "skipped_empty" };
      }

      const restoredAppState = restored.appState ?? {};
      const requestedBackground =
        typeof restoredAppState.viewBackgroundColor === "string"
          ? restoredAppState.viewBackgroundColor
          : "";
      const colorProbe = document.createElement("span");
      colorProbe.style.color = "";
      colorProbe.style.color = requestedBackground;
      const background = colorProbe.style.color ? requestedBackground : "#ffffff";

      const blob = await exportToBlob({
        elements,
        appState: {
          ...restoredAppState,
          exportBackground: true,
          exportWithDarkMode: false,
          viewBackgroundColor: background,
        },
        files: restored.files ?? {},
        mimeType: "image/png",
        getDimensions(width: number, height: number) {
          const dimensions = (window as ConverterWindow).__tangleafBoundedDimensions;
          if (!dimensions) {
            throw new Error("bounded dimensions policy is unavailable");
          }
          return dimensions(width, height);
        },
      });
      if (blob.type !== "image/png") {
        throw new Error("Excalidraw returned a non-PNG blob");
      }

      const blobUrl = URL.createObjectURL(blob);
      (window as ConverterWindow).__tangleafDrawingDispose = () => {
        URL.revokeObjectURL(blobUrl);
        delete (window as ConverterWindow).__tangleafDrawingDownload;
        delete (window as ConverterWindow).__tangleafDrawingDispose;
      };
      (window as ConverterWindow).__tangleafDrawingDownload = () => {
        const anchor = document.createElement("a");
        anchor.href = blobUrl;
        anchor.download = "drawing.png";
        document.body.append(anchor);
        anchor.click();
        anchor.remove();
        (window as ConverterWindow).__tangleafDrawingDispose?.();
      };
      return { status: "converted" };
    }, sourceDocument);

    if (prepared.status === "skipped_empty") {
      return prepared;
    }
    if (this.blockedRequests.length > 0) {
      await this.page.evaluate(() => (window as ConverterWindow).__tangleafDrawingDispose?.());
      return { status: "failed", blockedNetwork: true };
    }

    const downloadPromise = this.page.waitForEvent("download");
    await this.page.evaluate(() => (window as ConverterWindow).__tangleafDrawingDownload?.());
    const download = await downloadPromise;
    await download.saveAs(destination);
    if (this.blockedRequests.length > 0) {
      await fs.rm(destination, { force: true });
      return { status: "failed", blockedNetwork: true };
    }
    return { status: "converted" };
  }

  async close(): Promise<void> {
    await this.context.close();
    await this.browser.close();
  }
}
