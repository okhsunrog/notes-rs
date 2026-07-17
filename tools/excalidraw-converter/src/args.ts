import path from "node:path";

const SHA256 = /^[0-9a-f]{64}$/;

export interface ConverterOptions {
  sourceRoot: string;
  outputRoot: string;
  sourceManifestSha256: string;
  drawings: string[];
}

export function usage(): string {
  return `Usage: bun src/cli.ts \\
  --source-root <directory> \\
  --output <new-directory> \\
  --source-manifest-sha256 <lowercase-hex> \\
  --drawing <relative-path> [--drawing <relative-path> ...]`;
}

export function parseArguments(argv: string[]): ConverterOptions | { help: true } {
  const values: {
    sourceRoot: string | null;
    outputRoot: string | null;
    sourceManifestSha256: string | null;
    drawings: string[];
  } = {
    sourceRoot: null,
    outputRoot: null,
    sourceManifestSha256: null,
    drawings: [],
  };

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--help" || argument === "-h") {
      return { help: true };
    }

    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) {
      throw new Error(`missing value for ${argument}`);
    }
    index += 1;

    switch (argument) {
      case "--source-root":
        values.sourceRoot = value;
        break;
      case "--output":
        values.outputRoot = value;
        break;
      case "--source-manifest-sha256":
        values.sourceManifestSha256 = value;
        break;
      case "--drawing":
        values.drawings.push(value);
        break;
      default:
        throw new Error(`unknown argument: ${argument}`);
    }
  }

  if (!values.sourceRoot || !values.outputRoot || !values.sourceManifestSha256) {
    throw new Error("source root, output, and source manifest SHA-256 are required");
  }
  if (!SHA256.test(values.sourceManifestSha256)) {
    throw new Error("source manifest SHA-256 must be 64 lowercase hexadecimal characters");
  }
  if (values.drawings.length === 0) {
    throw new Error("at least one explicit drawing is required");
  }

  const sourceRoot = path.resolve(values.sourceRoot);
  const outputRoot = path.resolve(values.outputRoot);
  values.drawings = [...new Set(values.drawings.map(normalizeRelativePath))].sort((a, b) =>
    a.localeCompare(b, "en"),
  );
  return {
    sourceRoot,
    outputRoot,
    sourceManifestSha256: values.sourceManifestSha256,
    drawings: values.drawings,
  };
}

export function normalizeRelativePath(value: string): string {
  if (typeof value !== "string" || value.length === 0 || value.includes("\0")) {
    throw new Error("drawing path must be a non-empty string");
  }
  if (value.includes("\\")) {
    throw new Error("drawing path must use canonical forward slashes");
  }

  const normalized = value.normalize("NFC");
  if (normalized !== value) {
    throw new Error("drawing path must be Unicode NFC normalized");
  }
  if (path.posix.isAbsolute(normalized)) {
    throw new Error("drawing path must be relative");
  }
  if (Buffer.byteLength(normalized, "utf8") > 4096) {
    throw new Error("drawing path exceeds the bundle path limit");
  }

  const segments = normalized.split("/");
  if (segments.some((segment) => segment === "" || segment === "." || segment === "..")) {
    throw new Error("drawing path must not contain empty, dot, or parent segments");
  }
  if (!normalized.toLowerCase().endsWith(".excalidraw")) {
    throw new Error("drawing path must end with .excalidraw");
  }
  if (!normalized.startsWith("draws/")) {
    throw new Error("drawing path must be inside the canonical draws directory");
  }
  return normalized;
}
