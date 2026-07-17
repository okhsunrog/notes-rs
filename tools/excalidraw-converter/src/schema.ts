export type DrawingFailureCode =
  | "invalid_excalidraw_json"
  | "unsupported_excalidraw_schema"
  | "external_asset_blocked"
  | "resource_limit_exceeded"
  | "render_failed"
  | "output_encode_failed";

export interface SourceMetadata {
  relativePath: string;
  sizeBytes: number;
  sha256: string;
}

export interface OutputMetadata {
  relativePath: string;
  sizeBytes: number;
  sha256: string;
  width: number;
  height: number;
  mimeType: "image/png";
}

export type DrawingResult =
  | { source: SourceMetadata; status: "converted"; output: OutputMetadata }
  | { source: SourceMetadata; status: "skipped_empty" }
  | {
      source: SourceMetadata;
      status: "failed";
      error: { code: DrawingFailureCode; message: string };
    };

export interface ConversionBundle {
  schemaVersion: 1;
  converter: {
    name: string;
    version: string;
    excalidrawVersion: string;
    playwrightVersion: string;
    browserName: "chromium";
    browserVersion: string;
  };
  sourceRoot: {
    kind: "redacted";
    manifestSha256: string;
  };
  drawings: DrawingResult[];
}
