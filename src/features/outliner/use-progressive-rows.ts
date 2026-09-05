import { useEffect, useState } from "react";

export const ROW_BATCH_SIZE = 16;

/** Yield a paint between batches; retain all rows once initial mounting finishes. */
export function useProgressiveRows(
  total: number,
  enabled: boolean,
  scrollElement: HTMLElement | null = null,
) {
  const [limit, setLimit] = useState(0);
  useEffect(() => {
    if (limit === Infinity) return;
    if (!enabled || limit >= total) {
      setLimit(Infinity);
      return;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    let frame = 0;
    let touching = false;
    const cancel = () => {
      cancelAnimationFrame(frame);
      clearTimeout(timer);
    };
    const schedule = () => {
      frame = requestAnimationFrame(() => {
        timer = setTimeout(() => setLimit((value) => value + ROW_BATCH_SIZE), 0);
      });
    };
    const rest = () => {
      cancel();
      if (!touching) timer = setTimeout(schedule, 180);
    };
    const down = () => {
      touching = true;
      cancel();
    };
    const up = () => {
      touching = false;
      rest();
    };
    schedule();
    scrollElement?.addEventListener("scroll", rest, { passive: true });
    scrollElement?.addEventListener("pointerdown", down, { passive: true });
    window.addEventListener("pointerup", up, { passive: true });
    window.addEventListener("pointercancel", up, { passive: true });
    return () => {
      cancel();
      scrollElement?.removeEventListener("scroll", rest);
      scrollElement?.removeEventListener("pointerdown", down);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
    };
  }, [enabled, limit, total, scrollElement]);
  return enabled ? Math.min(total, limit) : total;
}
