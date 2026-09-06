import { FileText, PenLine } from "lucide-react";
import type { Page, PageKind } from "@/lib/api";

export type PageIconComponent = typeof FileText;

/** Handwritten notes are recognisable in every list before they are opened. */
export function pageIconFor(kind: PageKind): PageIconComponent {
  return kind.kind === "handwriting" ? PenLine : FileText;
}

export function PageIcon({ page, className }: { page: Page; className?: string }) {
  const Icon = pageIconFor(page.kind);
  return <Icon className={className} />;
}
