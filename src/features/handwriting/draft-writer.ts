import type { InkDraft } from "@/lib/bindings";

export type DraftSaveState = "saving" | "saved" | Error;

/** Serialize writes and retain the newest complete sheet when storage fails. */
export class DraftWriter {
  private pending: InkDraft | undefined;
  private running: Promise<boolean> | null = null;

  constructor(
    private revision: string | null,
    private readonly save: (draft: InkDraft, revision: string | null) => Promise<string>,
    private readonly onState: (state: DraftSaveState) => void,
  ) {}

  write(draft: InkDraft) {
    this.pending = draft;
    return this.flush();
  }

  flush(): Promise<boolean> {
    if (this.running) return this.running;
    this.running = this.drain().finally(() => {
      this.running = null;
    });
    return this.running;
  }

  private async drain() {
    while (this.pending) {
      const draft = this.pending;
      this.pending = undefined;
      this.onState("saving");
      try {
        this.revision = await this.save(draft, this.revision);
      } catch (error) {
        this.pending ??= draft;
        this.onState(error instanceof Error ? error : new Error(String(error)));
        return false;
      }
    }
    this.onState("saved");
    return true;
  }
}
