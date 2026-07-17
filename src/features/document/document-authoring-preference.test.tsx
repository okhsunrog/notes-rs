// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import {
  DEFAULT_DOCUMENT_AUTHORING_MODE,
  DOCUMENT_AUTHORING_PREFERENCE_KEY,
  DocumentAuthoringPreferenceStore,
  decodeDocumentAuthoringPreference,
  encodeDocumentAuthoringPreference,
  useDocumentAuthoringPreference,
} from "./document-authoring-preference";

afterEach(() => window.localStorage.clear());

function browserStore() {
  return new DocumentAuthoringPreferenceStore(() => ({
    storage: window.localStorage,
    addStorageListener: (listener) => window.addEventListener("storage", listener),
    removeStorageListener: (listener) => window.removeEventListener("storage", listener),
  }));
}

describe("document authoring preference", () => {
  it("defaults safely and strictly rejects corrupted or unknown payloads", () => {
    expect(decodeDocumentAuthoringPreference(null)).toBe("live_preview");
    expect(decodeDocumentAuthoringPreference("source")).toBe("live_preview");
    expect(decodeDocumentAuthoringPreference("{")).toBe("live_preview");
    expect(decodeDocumentAuthoringPreference('{"version":2,"mode":"source"}')).toBe("live_preview");
    expect(decodeDocumentAuthoringPreference('{"version":1,"mode":"preview"}')).toBe(
      "live_preview",
    );
    expect(
      decodeDocumentAuthoringPreference('{"version":1,"mode":"source","unexpected":true}'),
    ).toBe("live_preview");
    expect(decodeDocumentAuthoringPreference('{"version":1,"mode":"source"}')).toBe("source");
    expect(DEFAULT_DOCUMENT_AUTHORING_MODE).toBe("live_preview");
  });

  it("persists changes and notifies every same-window subscriber", () => {
    const store = browserStore();
    const first = vi.fn();
    const second = vi.fn();
    const unsubscribeFirst = store.subscribe(first);
    const unsubscribeSecond = store.subscribe(second);

    expect(store.getSnapshot()).toBe("live_preview");
    store.setMode("source");

    expect(store.getSnapshot()).toBe("source");
    expect(window.localStorage.getItem(DOCUMENT_AUTHORING_PREFERENCE_KEY)).toBe(
      encodeDocumentAuthoringPreference("source"),
    );
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);

    store.setMode("source");
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).toHaveBeenCalledTimes(1);
    unsubscribeFirst();
    unsubscribeSecond();

    window.localStorage.setItem(
      DOCUMENT_AUTHORING_PREFERENCE_KEY,
      encodeDocumentAuthoringPreference("live_preview"),
    );
    const remounted = vi.fn();
    const unsubscribeRemounted = store.subscribe(remounted);
    expect(store.getSnapshot()).toBe("live_preview");
    unsubscribeRemounted();
  });

  it("accepts cross-window StorageEvent updates and safe clear fallback", () => {
    const store = browserStore();
    const listener = vi.fn();
    const unsubscribe = store.subscribe(listener);

    window.dispatchEvent(
      new StorageEvent("storage", {
        key: DOCUMENT_AUTHORING_PREFERENCE_KEY,
        newValue: encodeDocumentAuthoringPreference("source"),
        storageArea: window.localStorage,
      }),
    );
    expect(store.getSnapshot()).toBe("source");
    expect(listener).toHaveBeenCalledTimes(1);

    window.dispatchEvent(
      new StorageEvent("storage", {
        key: DOCUMENT_AUTHORING_PREFERENCE_KEY,
        newValue: "corrupt",
        storageArea: window.localStorage,
      }),
    );
    expect(store.getSnapshot()).toBe("live_preview");
    expect(listener).toHaveBeenCalledTimes(2);

    store.setMode("source");
    window.dispatchEvent(new StorageEvent("storage", { key: null }));
    expect(store.getSnapshot()).toBe("live_preview");
    unsubscribe();
  });

  it("has a browser-free snapshot and tolerates unavailable storage", () => {
    const store = new DocumentAuthoringPreferenceStore(() => null);
    expect(store.getServerSnapshot()).toBe("live_preview");
    expect(store.getSnapshot()).toBe("live_preview");
    expect(() => store.setMode("source")).not.toThrow();
    expect(store.getSnapshot()).toBe("source");
  });

  it("projects same-window changes into every mounted hook consumer", async () => {
    function Probe({ name }: { name: string }) {
      const [mode, setMode] = useDocumentAuthoringPreference();
      return (
        <button type="button" data-probe={name} onClick={() => setMode("source")}>
          {mode}
        </button>
      );
    }

    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    try {
      await act(async () => {
        root.render(
          <>
            <Probe name="first" />
            <Probe name="second" />
          </>,
        );
      });
      expect(container.querySelector('[data-probe="first"]')?.textContent).toBe("live_preview");
      expect(container.querySelector('[data-probe="second"]')?.textContent).toBe("live_preview");

      await act(async () => {
        container.querySelector<HTMLButtonElement>('[data-probe="first"]')?.click();
      });
      expect(container.querySelector('[data-probe="first"]')?.textContent).toBe("source");
      expect(container.querySelector('[data-probe="second"]')?.textContent).toBe("source");
    } finally {
      act(() => root.unmount());
      container.remove();
    }
  });
});
