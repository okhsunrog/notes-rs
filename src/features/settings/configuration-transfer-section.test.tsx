// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, expect, it, vi } from "vite-plus/test";
import { ConfigurationTransferSection } from "./configuration-transfer-section";
import {
  applyConfigurationImport,
  cancelConfigurationImport,
  exportDeviceConfiguration,
  previewConfigurationImport,
} from "@/lib/api";

vi.mock("@/lib/api", () => ({
  applyConfigurationImport: vi.fn(),
  cancelConfigurationImport: vi.fn().mockResolvedValue(undefined),
  exportDeviceConfiguration: vi.fn().mockResolvedValue(true),
  previewConfigurationImport: vi.fn(),
  restartApp: vi.fn(),
}));

beforeEach(() => {
  vi.clearAllMocks();
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

async function mounted(run: (container: HTMLDivElement) => Promise<void>) {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () =>
      root.render(
        <ConfigurationTransferSection
          appearance={{ theme: "dark", palette: "iris" }}
          disabled={false}
          onBusyChange={vi.fn()}
          onImported={vi.fn()}
        />,
      ),
    );
    await run(container);
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
}

function button(container: HTMLElement, text: string) {
  return [...container.querySelectorAll("button")].find((element) =>
    element.textContent?.includes(text),
  )!;
}

it("exports no secrets or appearance unless the user explicitly enables them", async () => {
  await mounted(async (container) => {
    await act(async () => button(container, "Export configuration").click());
    expect(exportDeviceConfiguration).toHaveBeenLastCalledWith(false, null);
    await act(async () => {
      for (const checkbox of container.querySelectorAll<HTMLInputElement>('input[type="checkbox"]'))
        checkbox.click();
    });
    await act(async () => button(container, "Export configuration").click());
    expect(exportDeviceConfiguration).toHaveBeenLastCalledWith(true, {
      theme: "dark",
      palette: "iris",
    });
  });
});

it("requires review and a missing token before applying; cancel discards the staged import", async () => {
  vi.mocked(previewConfigurationImport).mockResolvedValue({
    id: "preview-id",
    serverUrl: "https://notes.example/",
    tokenIncluded: false,
    tokenRequired: true,
    search: { enabled: true, trigger: "enter_only", rerank: false },
    appearance: null,
  });
  await mounted(async (container) => {
    await act(async () => button(container, "Import configuration").click());
    expect(container.textContent).toContain("https://notes.example/");
    expect(container.textContent).toContain("Required for this server");
    expect(button(container, "Apply configuration").disabled).toBe(true);
    expect(applyConfigurationImport).not.toHaveBeenCalled();
    await act(async () => button(container, "Cancel import").click());
    expect(cancelConfigurationImport).toHaveBeenCalledWith("preview-id");
    expect(container.textContent).not.toContain("Import preview");
    expect(applyConfigurationImport).not.toHaveBeenCalled();
  });
});
