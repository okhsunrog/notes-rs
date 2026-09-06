// @vitest-environment jsdom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { beforeEach, expect, it } from "vite-plus/test";
import { AppearanceProvider } from "@/app/appearance";
import { DISPLAY_PROFILE_STORAGE_KEY } from "@/app/display-profile";
import { BusyIndicator } from "./busy-indicator";

beforeEach(() => {
  (
    globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
  localStorage.clear();
});

async function rendered(profile: "standard" | "eink", node: React.ReactNode) {
  localStorage.setItem(DISPLAY_PROFILE_STORAGE_KEY, profile);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => root.render(<AppearanceProvider>{node}</AppearanceProvider>));
  const html = container.innerHTML;
  await act(async () => root.unmount());
  container.remove();
  return html;
}

it("spins on a standard display", async () => {
  const html = await rendered("standard", <BusyIndicator label="Saving" />);
  expect(html).toContain("animate-spin");
  expect(html).toContain('aria-label="Saving"');
});

it("replaces the spinner with static text on e-ink", async () => {
  const html = await rendered("eink", <BusyIndicator label="Saving" />);
  expect(html).not.toContain("animate-spin");
  expect(html).toContain("Saving");
  expect(html).toContain('role="status"');
});

it("keeps a hidden label from repeating text the surrounding UI already shows", async () => {
  const html = await rendered("eink", <BusyIndicator label="Saving" hideLabel />);
  expect(html).not.toContain("animate-spin");
  expect(html).not.toContain(">Saving<");
  expect(html).toContain('aria-label="Saving"');
  expect(html).toContain("…");
});

it("falls back to a default label", async () => {
  const html = await rendered("eink", <BusyIndicator />);
  expect(html).toContain("Working…");
});
