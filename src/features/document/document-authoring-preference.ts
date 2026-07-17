import { useSyncExternalStore } from "react";
import type { DocumentAuthoringMode } from "./continuous-document-editor";

export const DOCUMENT_AUTHORING_PREFERENCE_KEY = "notes-rs.document-authoring-mode.v1";
export const DEFAULT_DOCUMENT_AUTHORING_MODE: DocumentAuthoringMode = "live_preview";

type PreferencePayload = {
  version: 1;
  mode: DocumentAuthoringMode;
};

type PreferenceEnvironment = {
  storage: Pick<Storage, "getItem" | "setItem">;
  addStorageListener: (listener: (event: StorageEvent) => void) => void;
  removeStorageListener: (listener: (event: StorageEvent) => void) => void;
};

type EnvironmentResolver = () => PreferenceEnvironment | null;

function isAuthoringMode(value: unknown): value is DocumentAuthoringMode {
  return value === "live_preview" || value === "source";
}

export function decodeDocumentAuthoringPreference(raw: string | null): DocumentAuthoringMode {
  if (raw === null) return DEFAULT_DOCUMENT_AUTHORING_MODE;
  try {
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return DEFAULT_DOCUMENT_AUTHORING_MODE;
    }
    const keys = Object.keys(value).sort();
    if (keys.length !== 2 || keys[0] !== "mode" || keys[1] !== "version") {
      return DEFAULT_DOCUMENT_AUTHORING_MODE;
    }
    const payload = value as Partial<PreferencePayload>;
    return payload.version === 1 && isAuthoringMode(payload.mode)
      ? payload.mode
      : DEFAULT_DOCUMENT_AUTHORING_MODE;
  } catch {
    return DEFAULT_DOCUMENT_AUTHORING_MODE;
  }
}

export function encodeDocumentAuthoringPreference(mode: DocumentAuthoringMode): string {
  return JSON.stringify({ version: 1, mode } satisfies PreferencePayload);
}

function browserEnvironment(): PreferenceEnvironment | null {
  if (typeof window === "undefined") return null;
  try {
    const storage = window.localStorage;
    return {
      storage,
      addStorageListener: (listener) => window.addEventListener("storage", listener),
      removeStorageListener: (listener) => window.removeEventListener("storage", listener),
    };
  } catch {
    return null;
  }
}

export class DocumentAuthoringPreferenceStore {
  private readonly listeners = new Set<() => void>();
  private initialized = false;
  private listening = false;
  private mode: DocumentAuthoringMode = DEFAULT_DOCUMENT_AUTHORING_MODE;
  private environment: PreferenceEnvironment | null = null;

  constructor(private readonly resolveEnvironment: EnvironmentResolver = browserEnvironment) {}

  readonly getSnapshot = (): DocumentAuthoringMode => {
    this.initialize();
    return this.mode;
  };

  readonly getServerSnapshot = (): DocumentAuthoringMode => DEFAULT_DOCUMENT_AUTHORING_MODE;

  readonly subscribe = (listener: () => void): (() => void) => {
    this.initialize();
    if (this.listeners.size === 0) this.refreshFromStorage();
    this.listeners.add(listener);
    this.startListening();
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size === 0) this.stopListening();
    };
  };

  readonly setMode = (mode: DocumentAuthoringMode): void => {
    this.initialize();
    try {
      this.environment?.storage.setItem(
        DOCUMENT_AUTHORING_PREFERENCE_KEY,
        encodeDocumentAuthoringPreference(mode),
      );
    } catch {
      // A denied or full localStorage must not make the editor unusable.
    }
    this.update(mode);
  };

  private readonly onStorage = (event: StorageEvent): void => {
    if (event.storageArea && event.storageArea !== this.environment?.storage) return;
    if (event.key !== DOCUMENT_AUTHORING_PREFERENCE_KEY && event.key !== null) return;
    this.update(decodeDocumentAuthoringPreference(event.key === null ? null : event.newValue));
  };

  private initialize(): void {
    if (this.initialized) return;
    this.initialized = true;
    this.environment = this.resolveEnvironment();
    this.refreshFromStorage();
  }

  private refreshFromStorage(): void {
    try {
      this.mode = decodeDocumentAuthoringPreference(
        this.environment?.storage.getItem(DOCUMENT_AUTHORING_PREFERENCE_KEY) ?? null,
      );
    } catch {
      this.mode = DEFAULT_DOCUMENT_AUTHORING_MODE;
    }
  }

  private startListening(): void {
    if (this.listening || !this.environment) return;
    this.environment.addStorageListener(this.onStorage);
    this.listening = true;
  }

  private stopListening(): void {
    if (!this.listening || !this.environment) return;
    this.environment.removeStorageListener(this.onStorage);
    this.listening = false;
  }

  private update(mode: DocumentAuthoringMode): void {
    if (mode === this.mode) return;
    this.mode = mode;
    for (const listener of this.listeners) listener();
  }
}

const documentAuthoringPreferenceStore = new DocumentAuthoringPreferenceStore();

export function useDocumentAuthoringPreference(): readonly [
  DocumentAuthoringMode,
  (mode: DocumentAuthoringMode) => void,
] {
  const mode = useSyncExternalStore(
    documentAuthoringPreferenceStore.subscribe,
    documentAuthoringPreferenceStore.getSnapshot,
    documentAuthoringPreferenceStore.getServerSnapshot,
  );
  return [mode, documentAuthoringPreferenceStore.setMode] as const;
}
