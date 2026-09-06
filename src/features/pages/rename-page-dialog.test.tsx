// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vite-plus/test";

const renamePage = vi.fn();
vi.mock("@/lib/api", () => ({
  CommandFailure: class CommandFailure extends Error {
    code = "internal";
  },
  getPage: vi.fn(),
  renamePage: (...args: unknown[]) => renamePage(...args),
}));

import { PageSessionProvider } from "./page-session";
import { RenamePageDialog } from "./rename-page-dialog";
import type { Page } from "@/lib/api";

const page: Page = {
  uuid: "11111111-1111-4111-8111-111111111111",
  kind: { kind: "handwriting" },
  title: "Untitled note",
  layout: "outline",
  titleRevision: "1",
  createdAt: 0,
  updatedAt: 0,
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.useFakeTimers();
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  renamePage.mockReset();
  renamePage.mockImplementation(async (_uuid: string, title: string | null) => ({
    ...page,
    title,
    titleRevision: "2",
  }));
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
});

function type(input: HTMLInputElement, value: string) {
  // React tracks the value setter; bypass it so the change event carries the new text.
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

it("keeps a trailing space while typing and renames only on save", async () => {
  await act(async () =>
    root.render(
      <PageSessionProvider>
        <RenamePageDialog page={page} open onOpenChange={() => {}} onSaved={() => {}} />
      </PageSessionProvider>,
    ),
  );
  const input = document.querySelector('input[aria-label="Note name"]') as HTMLInputElement;
  await act(async () => type(input, "test "));
  // The autosave pause of the shared title editor must not trim the draft under the user.
  await act(async () => {
    vi.advanceTimersByTime(1000);
  });
  expect(input.value).toBe("test ");
  expect(renamePage).not.toHaveBeenCalled();
  await act(async () => type(input, "test note"));
  await act(async () => {
    input.form!.requestSubmit();
  });
  await act(async () => {
    await Promise.resolve();
  });
  expect(renamePage).toHaveBeenCalledTimes(1);
  expect(renamePage.mock.calls[0]![1]).toBe("test note");
});
