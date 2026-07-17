import { DatabaseBackup, Download, FolderOpen, Upload } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { SettingsSnapshot } from "@/lib/api";
import { Field, SettingsSection } from "./settings-controls";

type Props = {
  settings: SettingsSnapshot;
  dataAvailable: boolean;
  busy: boolean;
  updateSyncDirectory: (directory: string | null) => void;
  dataAction: (action: "export" | "import" | "backup") => Promise<void>;
  chooseSync: () => Promise<void>;
  runSync: (direction: "push" | "pull") => Promise<void>;
};

export function DataSettingsSections({
  settings,
  dataAvailable,
  busy,
  updateSyncDirectory,
  dataAction,
  chooseSync,
  runSync,
}: Props) {
  return (
    <>
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
      </SettingsSection>

      <SettingsSection
        title="Folder sync"
        description="Manual, conflict-safe sync through a folder managed by Syncthing, Nextcloud, Dropbox, or another file synchronizer. Push writes one atomic snapshot; pull always creates a local recovery backup first."
      >
        <Field label="Sync directory">
          <div className="flex gap-2">
            <Input
              value={settings.syncDirectory ?? ""}
              onChange={(event) => updateSyncDirectory(event.currentTarget.value || null)}
              placeholder="Choose a directory…"
            />
            <Button
              type="button"
              variant="outline"
              aria-label="Choose sync directory"
              onClick={() => void chooseSync()}
            >
              <FolderOpen className="size-4" />
            </Button>
          </div>
        </Field>
        <div className="flex flex-wrap gap-2">
          <Button
            type="button"
            variant="outline"
            disabled={!dataAvailable || busy || !settings.syncDirectory?.trim()}
            onClick={() => void runSync("push")}
          >
            <Upload className="size-4" /> Push snapshot
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={!dataAvailable || busy || !settings.syncDirectory?.trim()}
            onClick={() => void runSync("pull")}
          >
            <Download className="size-4" /> Pull snapshot
          </Button>
        </div>
      </SettingsSection>
    </>
  );
}
