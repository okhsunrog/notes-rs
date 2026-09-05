import { useEffect, useState } from "react";

export const ROW_BATCH_SIZE = 16;

/** Yield a paint between batches; retain all rows once initial mounting finishes. */
export function useProgressiveRows(total: number, enabled: boolean) {
  const [limit, setLimit] = useState(0);
  useEffect(() => {
    if (!enabled || limit === Infinity) return;
    if (limit >= total) {
      setLimit(Infinity);
      return;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    const frame = requestAnimationFrame(() => {
      timer = setTimeout(() => setLimit((value) => value + ROW_BATCH_SIZE), 0);
    });
    return () => {
      cancelAnimationFrame(frame);
      clearTimeout(timer);
    };
  }, [enabled, limit, total]);
  return enabled ? Math.min(total, limit) : total;
}
