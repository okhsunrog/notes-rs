import { DatabaseBackup, Download, Upload } from "lucide-react";
import { Button } from "@/components/ui/button";
import { LogseqImportSection } from "./logseq-import-section";
import { SettingsSection } from "./settings-controls";

type Props = {
  dataAvailable: boolean;
  busy: boolean;
  dataAction: (action: "export" | "import" | "backup") => Promise<void>;
  onDataChanged: (openPageUuid: string | null) => void;
  onError: (message: string) => void;
  onMessage: (message: string) => void;
};

export function DataSettingsSections({
  dataAvailable,
  busy,
  dataAction,
  onDataChanged,
  onError,
  onMessage,
}: Props) {
  return (
    <SettingsSection
      title="Data"
      description="Portable JSON archives include every note, graph edge, and attachment. Destructive operations automatically create a timestamped recovery backup in app data."
    >
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="outline"
          disabled={!dataAvailable || busy}
          onClick={() => void dataAction("export")}
        >
          <Download className="size-4" /> Export archive
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={!dataAvailable || busy}
          onClick={() => void dataAction("import")}
        >
          <Upload className="size-4" /> Import archive
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={!dataAvailable || busy}
          onClick={() => void dataAction("backup")}
        >
          <DatabaseBackup className="size-4" /> Create backup
        </Button>
      </div>
      {!dataAvailable && (
        <p className="text-xs text-muted-foreground">
          Data tools become available after the database starts successfully.
        </p>
      )}
      <LogseqImportSection
        disabled={!dataAvailable || busy}
        onDataChanged={onDataChanged}
        onError={onError}
        onMessage={onMessage}
      />
    </SettingsSection>
  );
}
