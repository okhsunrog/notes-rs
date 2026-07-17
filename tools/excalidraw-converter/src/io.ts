import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

export const MAX_SOURCE_BYTES = 64 * 1024 * 1024;

export function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function isWithin(parent: string, child: string): boolean {
  const relative = path.relative(parent, child);
  return relative === "" || (!relative.startsWith(`..${path.sep}`) && relative !== "..");
}

export async function validateRoots(sourceRoot: string, outputRoot: string) {
  const canonicalSource = await fs.realpath(sourceRoot);
  const sourceStat = await fs.stat(canonicalSource);
  if (!sourceStat.isDirectory()) {
    throw new Error("source root is not a directory");
  }

  const outputParent = await fs.realpath(path.dirname(outputRoot));
  const canonicalOutput = path.join(outputParent, path.basename(outputRoot));
  if (isWithin(canonicalSource, canonicalOutput) || isWithin(canonicalOutput, canonicalSource)) {
    throw new Error("output must be disjoint from the source root");
  }

  try {
    await fs.lstat(canonicalOutput);
    throw new Error("output already exists");
  } catch (error: unknown) {
    if (!isNodeError(error) || error.code !== "ENOENT") {
      throw error;
    }
  }

  return { sourceRoot: canonicalSource, outputRoot: canonicalOutput };
}

export async function readSourceDrawing(
  sourceRoot: string,
  relativePath: string,
): Promise<
  | { bytes: null; sizeBytes: number; sha256: string; tooLarge: true }
  | { bytes: Buffer; sizeBytes: number; sha256: string; tooLarge: false }
> {
  const candidate = path.join(sourceRoot, ...relativePath.split("/"));
  const handle = await fs.open(candidate, "r");
  try {
    const openedPath = await canonicalOpenedPath(handle, candidate);
    if (!isWithin(sourceRoot, openedPath)) {
      throw new Error("drawing resolves outside the source root");
    }

    const stat = await handle.stat();
    if (!stat.isFile()) {
      throw new Error("drawing is not a regular file");
    }
    if (stat.size > MAX_SOURCE_BYTES) {
      return {
        bytes: null,
        sizeBytes: stat.size,
        sha256: await hashOpenFile(handle),
        tooLarge: true,
      };
    }

    const bytes = await handle.readFile();
    return {
      bytes,
      sizeBytes: bytes.byteLength,
      sha256: sha256(bytes),
      tooLarge: false,
    };
  } finally {
    await handle.close();
  }
}

async function canonicalOpenedPath(handle: fs.FileHandle, fallbackPath: string): Promise<string> {
  if (process.platform === "linux") {
    return fs.realpath(`/proc/self/fd/${handle.fd}`);
  }
  return fs.realpath(fallbackPath);
}

async function hashOpenFile(handle: fs.FileHandle): Promise<string> {
  const hash = createHash("sha256");
  const buffer = Buffer.allocUnsafe(1024 * 1024);
  let position = 0;
  for (;;) {
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, position);
    if (bytesRead === 0) {
      break;
    }
    hash.update(buffer.subarray(0, bytesRead));
    position += bytesRead;
  }
  return hash.digest("hex");
}

export async function createPublication(outputRoot: string) {
  const stagingRoot = `${outputRoot}.tmp-${process.pid}-${randomUUID()}`;
  await fs.mkdir(path.join(stagingRoot, "artifacts"), { recursive: true, mode: 0o700 });
  let published = false;

  return {
    stagingRoot,
    async publish(bundle: unknown) {
      const bundlePath = path.join(stagingRoot, "conversion-bundle.json");
      await fs.writeFile(bundlePath, `${JSON.stringify(bundle, null, 2)}\n`, {
        encoding: "utf8",
        mode: 0o600,
        flag: "wx",
      });
      await fs.rename(stagingRoot, outputRoot);
      published = true;
    },
    async discard() {
      if (!published) {
        await fs.rm(stagingRoot, { recursive: true, force: true });
      }
    },
  };
}

export async function installPng(
  stagingRoot: string,
  temporaryPath: string,
  sourceRelativePath: string,
) {
  const bytes = await fs.readFile(temporaryPath);
  const dimensions = readPngDimensions(bytes);
  const digest = sha256(bytes);
  const sourceIdentity = sha256(Buffer.from(sourceRelativePath, "utf8"));
  const relativePath = `artifacts/${digest.slice(0, 2)}/${sourceIdentity}-${digest}.png`;
  const destination = path.join(stagingRoot, ...relativePath.split("/"));
  await fs.mkdir(path.dirname(destination), { recursive: true, mode: 0o700 });

  try {
    await fs.rename(temporaryPath, destination);
  } catch (error: unknown) {
    if (!isNodeError(error) || error.code !== "EEXIST") {
      throw error;
    }
    const existing = await fs.readFile(destination);
    if (sha256(existing) !== digest) {
      throw new Error("artifact hash collision");
    }
    await fs.rm(temporaryPath, { force: true });
  }

  return {
    relativePath,
    sizeBytes: bytes.byteLength,
    sha256: digest,
    width: dimensions.width,
    height: dimensions.height,
    mimeType: "image/png" as const,
  };
}

export function readPngDimensions(bytes: Buffer): { width: number; height: number } {
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  if (bytes.byteLength < 24 || !bytes.subarray(0, 8).equals(signature)) {
    throw new Error("export did not produce a valid PNG");
  }
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  if (width === 0 || height === 0 || width > 8192 || height > 8192) {
    throw new Error("exported PNG dimensions are invalid");
  }
  if (width * height > 25_000_000) {
    throw new Error("exported PNG exceeds the pixel budget");
  }
  return { width, height };
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error;
}
