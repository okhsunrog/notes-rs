import { createContext, useContext, type ReactNode } from "react";
import type { MarkdownOpenHandler } from "@/features/markdown";
import type { Content, JournalDate, Page } from "@/lib/api";
import type { OpenDisposition } from "./workspace-model";

export type WorkspaceController = {
  createNewNote: (title?: string, disposition?: OpenDisposition) => void | Promise<void>;
  openContent: (content: Content, disposition?: OpenDisposition) => void | Promise<void>;
  openJournal: (date: JournalDate, disposition?: OpenDisposition) => void | Promise<void>;
  captureJournal: (
    date: JournalDate,
    markdown: string,
    openAfterCapture?: boolean,
  ) => Promise<boolean>;
  onSaved: (page: Page) => void;
  onDelete: (page: Page) => void | Promise<void>;
  openMarkdownLink: MarkdownOpenHandler;
};

export const WorkspaceControllerContext = createContext<WorkspaceController | null>(null);

export function WorkspaceControllerProvider({
  controller,
  children,
}: {
  controller: WorkspaceController;
  children: ReactNode;
}) {
  return (
    <WorkspaceControllerContext.Provider value={controller}>
      {children}
    </WorkspaceControllerContext.Provider>
  );
}

export function useWorkspaceController(): WorkspaceController {
  const controller = useContext(WorkspaceControllerContext);
  if (!controller) {
    throw new Error("useWorkspaceController must be used inside WorkspaceControllerProvider");
  }
  return controller;
}
