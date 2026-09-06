import type { InkDraft } from "@/lib/bindings";

export type DraftSaveState = "saving" | "saved" | Error;

/** Serialize writes and retain every gesture when storage fails. */
export class DraftWriter {
  private pending: InkDraft[] = [];
  private running: Promise<boolean> | null = null;
  private failure: Error | null = null;

  constructor(
    private revision: string | null,
    private readonly save: (draft: InkDraft, revision: string | null) => Promise<string>,
    private onState: (state: DraftSaveState) => void,
  ) {}

  /** Adopt the writer in a new mount without losing what it still owes. */
  setOnState(onState: (state: DraftSaveState) => void) {
    this.onState = onState;
  }

  hasPending() {
    return this.pending.length > 0;
  }

  /**
   * Why the last flush stopped. Callers distinguish a note that has been
   * deleted — nothing is left to store, so the session can be closed — from a
   * storage failure whose gestures must be kept for a retry.
   */
  lastError(): Error | null {
    return this.failure;
  }

  /** The newest queued state: never older than what storage would return. */
  latestDraft(): InkDraft | null {
    return this.pending.length ? this.pending[this.pending.length - 1]! : null;
  }

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
        this.failure = error instanceof Error ? error : new Error(String(error));
        this.onState(this.failure);
        return false;
      }
    }
    this.failure = null;
    this.onState("saved");
    return true;
  }
}
