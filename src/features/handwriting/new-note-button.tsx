import type { ReactNode } from "react";
import { Menu } from "@base-ui/react/menu";
import { ChevronDown, PenLine } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { refreshPanelAfterClose } from "@/app/eink-refresh";
import { useOverlayInkSuppression } from "@/app/ink-suppression";
import { useWorkspaceController } from "@/features/workspace/workspace-controller";
import { useHandwritingAvailability } from "./input-capabilities";

export function NewNoteButton({
  children,
  onClick,
  disabled,
  className,
  fullWidth = false,
}: {
  children: ReactNode;
  onClick: () => void;
  disabled?: boolean;
  className?: string;
  fullWidth?: boolean;
}) {
  const { available } = useHandwritingAvailability();
  const controller = useWorkspaceController();
  const overlay = useOverlayInkSuppression();
  return (
    <div className={cn("inline-flex min-w-0", fullWidth && "w-full")}>
      <Button
        type="button"
        disabled={disabled}
        onClick={onClick}
        className={cn(className, fullWidth && "flex-1", available && "rounded-r-none")}
      >
        {children}
      </Button>
      {available && (
        <Menu.Root onOpenChange={refreshPanelAfterClose(overlay)}>
          <Menu.Trigger
            render={
              <Button
                type="button"
                disabled={disabled}
                aria-label="More note options"
                className="brand-button h-9 w-9 shrink-0 rounded-xl rounded-l-none border-l border-primary-foreground/25 px-0"
              />
            }
          >
            <ChevronDown className="size-4" />
          </Menu.Trigger>
          <Menu.Portal>
            <Menu.Positioner sideOffset={6} align="end">
              <Menu.Popup className="z-50 min-w-48 rounded-xl border bg-popover p-1 text-popover-foreground shadow-panel outline-none">
                <Menu.Item
                  className="flex cursor-pointer items-center gap-2 rounded-lg px-3 py-2.5 text-sm outline-none data-[highlighted]:bg-accent"
                  onClick={() => void controller.createHandwrittenNote()}
                >
                  <PenLine className="size-4" />
                  Write by hand
                </Menu.Item>
              </Menu.Popup>
            </Menu.Positioner>
          </Menu.Portal>
        </Menu.Root>
      )}
    </div>
  );
}
