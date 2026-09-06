import { Loader2 } from "lucide-react";
import { useResolvedDisplay } from "@/app/appearance";
import { cn } from "@/lib/utils";

type Props = {
  /** What is happening. Always the accessible name; also the visible text on e-ink. */
  label?: string;
  className?: string;
  /**
   * Set where the surrounding UI already says what is happening (a button caption, a status
   * line). E-ink then shows a static ellipsis instead of repeating the label.
   */
  hideLabel?: boolean;
};

/**
 * A spinner everywhere except e-ink, where every rotation frame is a full panel refresh: the
 * indicator becomes static text there instead of animating.
 */
export function BusyIndicator({ label = "Working…", className, hideLabel = false }: Props) {
  const display = useResolvedDisplay();

  if (display === "eink") {
    return (
      <span
        role="status"
        aria-label={hideLabel ? label : undefined}
        className={cn("inline-flex items-center whitespace-nowrap tabular-nums", className)}
      >
        {hideLabel ? "…" : label}
      </span>
    );
  }

  return <Loader2 role="status" aria-label={label} className={cn("animate-spin", className)} />;
}
