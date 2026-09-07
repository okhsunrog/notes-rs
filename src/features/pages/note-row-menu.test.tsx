// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import {
  WorkspaceControllerProvider,
  type WorkspaceController,
} from "@/features/workspace/workspace-controller";
import type { Page } from "@/lib/api";
import { NoteRowMenu } from "./note-row-menu";
import { PageSessionProvider } from "./page-session";
import { usePageNavigationStore } from "./page-navigation-store";

vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>();
  return { ...actual, getPage: vi.fn(async () => null), renamePage: vi.fn() };
});
vi.mock("@/lib/notify", () => ({
  notifyError: vi.fn(),
  notifyInfo: vi.fn(),
  notifySuccess: vi.fn(),
  notifyRetryableError: vi.fn(),
}));

const page: Page = {
  uuid: "019f0000-0000-7000-8000-00000000000a",
  kind: { kind: "handwriting" },
  title: "Field notes",
  layout: "outline",
  titleRevision: "1",
  createdAt: 0,
  updatedAt: 0,
};

const onDelete = vi.fn();
const controller: WorkspaceController = {
  createNewNote: () => undefined,
  createHandwrittenNote: () => undefined,
  openAllNotes: () => undefined,
  openContent: () => undefined,
  openJournal: () => undefined,
  captureJournal: async () => true,
  onSaved: () => undefined,
  onDelete,
  openMarkdownLink: () => undefined,
};

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
});

beforeEach(() => {
  onDelete.mockReset();
  usePageNavigationStore.setState({ recentPageUuids: [], favoritePageUuids: [] });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

/** The row is a button too, so the harness records anything that reaches it. */
const openRow = vi.fn();

async function mount() {
  openRow.mockReset();
  await act(async () =>
    root.render(
      <PageSessionProvider>
        <WorkspaceControllerProvider controller={controller}>
          <div onClick={() => openRow()}>
            <NoteRowMenu page={page} />
          </div>
        </WorkspaceControllerProvider>
      </PageSessionProvider>,
    ),
  );
}

function trigger() {
  const found = document.querySelector<HTMLElement>(
    'button[aria-label="More options for Field notes"]',
  );
  if (!found) throw new Error("no menu trigger");
  return found;
}

/** The popup is portalled out of the row, so items are looked up in the document. */
function menuItem(label: string) {
  const found = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
    (element) => element.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no menu item labelled ${label}`);
  return found;
}

async function openMenu() {
  await act(async () => {
    trigger().click();
  });
}

it("opens without also opening the row it sits in", async () => {
  await mount();
  await openMenu();

  expect(openRow).not.toHaveBeenCalled();
  // A 32px target: the pen and a fingertip both have to be able to hit it.
  expect(trigger().dataset.size).toBe("icon-sm");
  expect(menuItem("Rename")).toBeTruthy();
});

it("renames from the row, without opening the note", async () => {
  await mount();
  await openMenu();
  await act(async () => {
    menuItem("Rename").click();
  });

  expect(document.querySelector('input[aria-label="Note name"]')).toBeTruthy();
  expect(openRow).not.toHaveBeenCalled();
});

it("toggles the favorite both ways", async () => {
  await mount();
  await openMenu();
  await act(async () => {
    menuItem("Add to favorites").click();
  });
  expect(usePageNavigationStore.getState().favoritePageUuids).toEqual([page.uuid]);

  await openMenu();
  await act(async () => {
    menuItem("Remove from favorites").click();
  });
  expect(usePageNavigationStore.getState().favoritePageUuids).toEqual([]);
});

it("hands the delete to the workspace controller, which confirms it", async () => {
  await mount();
  await openMenu();
  await act(async () => {
    menuItem("Delete note").click();
  });

  expect(onDelete).toHaveBeenCalledWith(page);
});
