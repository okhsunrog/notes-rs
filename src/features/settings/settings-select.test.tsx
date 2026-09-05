// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vite-plus/test";
import { SettingsSelect } from "./settings-select";

it("shows labels for numeric values and selects through the themed popup", async () => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const change = vi.fn();
  try {
    await act(async () =>
      root.render(
        <SettingsSelect
          label="Radius"
          value={10}
          onValueChange={change}
          options={[
            { value: 10, label: "10 px (default)" },
            { value: 0, label: "Square" },
          ]}
        />,
      ),
    );
    const trigger = container.querySelector<HTMLButtonElement>('[role="combobox"]')!;
    expect(trigger.textContent).toContain("10 px (default)");
    expect(trigger.getAttribute("aria-label")).toBe("Radius");
    await act(async () => trigger.click());
    const options = [...document.querySelectorAll<HTMLElement>('[role="option"]')];
    expect(options).toHaveLength(2);
    expect(container.contains(options[0])).toBe(false);
    await act(async () =>
      options.find((option) => option.textContent?.includes("Square"))!.click(),
    );
    expect(change).toHaveBeenCalledWith(0);
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});

it("shows the empty placeholder and disables unavailable choices", async () => {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () =>
      root.render(
        <SettingsSelect
          label="Startup note"
          value={null}
          options={[]}
          disabled
          placeholder="No notes available"
          onValueChange={vi.fn()}
        />,
      ),
    );
    const trigger = container.querySelector<HTMLButtonElement>('[role="combobox"]')!;
    expect(trigger.disabled).toBe(true);
    expect(trigger.textContent).toContain("No notes available");
  } finally {
    await act(async () => root.unmount());
    container.remove();
  }
});
