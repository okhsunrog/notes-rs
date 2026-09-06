import type { InkDraft } from "@/lib/bindings";

export type DraftSaveState = "saving" | "saved" | Error;

/** Serialize writes and retain every gesture when storage fails. */
export class DraftWriter {
  private pending: InkDraft[] = [];
  private running: Promise<boolean> | null = null;

  constructor(
    private revision: string | null,
    private readonly save: (draft: InkDraft, revision: string | null) => Promise<string>,
    private readonly onState: (state: DraftSaveState) => void,
  ) {}

  write(draft: InkDraft) {
    this.pending.push(draft);
    // Keep the in-flight/retry head plus all states needed for the last 50 Undo steps.
    // Older queued intermediates may expire just like already-persisted history.
    if (this.pending.length > 52) this.pending.splice(1, this.pending.length - 52);
    return this.flush();
  }

  getRevision() {
    return this.revision;
  }

  flush(): Promise<boolean> {
    if (this.running) return this.running;
    this.running = this.drain().finally(() => {
      this.running = null;
    });
    return this.running;
  }

  private async drain() {
    while (this.pending.length) {
      const draft = this.pending[0]!;
      this.onState("saving");
      try {
        this.revision = await this.save(draft, this.revision);
        this.pending.shift();
      } catch (error) {
        this.onState(error instanceof Error ? error : new Error(String(error)));
        return false;
      }
    }
    this.onState("saved");
    return true;
  }
}
