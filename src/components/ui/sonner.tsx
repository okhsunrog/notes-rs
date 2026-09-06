import { CircleCheckIcon, InfoIcon, OctagonXIcon, TriangleAlertIcon } from "lucide-react";
import { useTheme } from "next-themes";
import { Toaster as Sonner, type ToasterProps } from "sonner";
import { useResolvedDisplay } from "@/app/appearance";
import { BusyIndicator } from "@/components/ui/busy-indicator";

/** A toast that fades in and out costs several panel refreshes, so e-ink shows it longer. */
const EINK_TOAST_DURATION_MS = 6_000;

const Toaster = ({ ...props }: ToasterProps) => {
  const { theme = "system" } = useTheme();
  const display = useResolvedDisplay();

  return (
    <Sonner
      theme={theme as ToasterProps["theme"]}
      duration={display === "eink" ? EINK_TOAST_DURATION_MS : undefined}
      className="toaster group"
      icons={{
        success: <CircleCheckIcon className="size-4" />,
        info: <InfoIcon className="size-4" />,
        warning: <TriangleAlertIcon className="size-4" />,
        error: <OctagonXIcon className="size-4" />,
        loading: <BusyIndicator className="size-4" label="Working" hideLabel />,
      }}
      style={
        {
          "--normal-bg": "var(--popover)",
          "--normal-text": "var(--popover-foreground)",
          "--normal-border": "var(--border)",
          "--border-radius": "var(--radius)",
        } as React.CSSProperties
      }
      {...props}
    />
  );
};

export { Toaster };
