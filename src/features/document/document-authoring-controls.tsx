import { BookOpen, Code2, FilePenLine, LockKeyhole, type LucideIcon } from "lucide-react";
import { useId } from "react";
import { Button } from "@/components/ui/button";
import type { PageLayout } from "@/lib/api";
import { PagePresentation } from "@/features/pages/page-presentation";
import type { DocumentAuthoringMode } from "./continuous-document-editor";
import {
  activeDocumentAuthoringAction,
  DocumentAuthoringAvailability,
  type DocumentAuthoringAction,
  isDocumentAuthoringActionEnabled,
} from "./document-authoring-model";

type Props = {
  layout: PageLayout;
  presentation: PagePresentation;
  authoringMode: DocumentAuthoringMode;
  availability: DocumentAuthoringAvailability;
  onAction: (action: DocumentAuthoringAction) => void;
};

const ACTIONS: ReadonlyArray<{
  value: DocumentAuthoringAction;
  label: string;
  icon: LucideIcon;
  authoringMode?: DocumentAuthoringMode;
}> = [
  { value: "write", label: "Write", icon: FilePenLine, authoringMode: "live_preview" },
  { value: "source", label: "Source", icon: Code2, authoringMode: "source" },
  { value: "read", label: "Read", icon: BookOpen },
];

export function DocumentAuthoringControls({
  layout,
  presentation,
  authoringMode,
  availability,
  onAction,
}: Props) {
  const explanationId = useId();
  if (layout !== "document") return null;
  const active = activeDocumentAuthoringAction(presentation, authoringMode);
  const unavailable = availability === DocumentAuthoringAvailability.WriterInOtherPane;
  return (
    <div className="flex min-w-0 items-center gap-2">
      <div
        role="group"
        aria-label="Document view"
        aria-describedby={unavailable ? explanationId : undefined}
        className="flex items-center rounded-lg border border-border/60 surface-card p-0.5"
      >
        {ACTIONS.map(({ value, label, icon: Icon, authoringMode: actionMode }) => {
          const selected = active === value;
          const returnMode =
            presentation === PagePresentation.Reading && actionMode === authoringMode;
          const enabled = isDocumentAuthoringActionEnabled(value, availability);
          return (
            <Button
              key={value}
              type="button"
              variant={selected ? "secondary" : "ghost"}
              size="xs"
              disabled={!enabled}
              aria-pressed={selected}
              aria-label={returnMode ? `${label}, preferred when leaving Read` : label}
              aria-describedby={!enabled ? explanationId : undefined}
              data-document-action={value}
              data-return-mode={returnMode || undefined}
              onClick={() => onAction(value)}
              className={
                returnMode
                  ? "relative rounded-md px-2 text-[10px] text-primary"
                  : "relative rounded-md px-2 text-[10px]"
              }
            >
              <Icon className="size-3" />
              <span className="hidden sm:inline">{label}</span>
              {returnMode && (
                <span
                  aria-hidden="true"
                  className="absolute -top-0.5 -right-0.5 size-1.5 rounded-full bg-primary"
                />
              )}
            </Button>
          );
        })}
      </div>
      {unavailable && (
        <span
          id={explanationId}
          role="status"
          className="inline-flex min-w-0 items-center gap-1 text-[10px] text-muted-foreground"
          title="This page is being edited in another pane"
        >
          <LockKeyhole aria-hidden="true" className="size-3 shrink-0" />
          <span className="truncate">Editing in another pane</span>
        </span>
      )}
    </div>
  );
}
