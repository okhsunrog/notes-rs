import { useCallback, useEffect, useRef, useState } from "react";
import { registerBackOverlay } from "@/lib/back-overlays";
import { Maximize2, RotateCcw, ZoomIn, ZoomOut } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import type { MarkdownResolvedImage } from "./image-policy";

type Point = { x: number; y: number };
type Transform = { scale: number; x: number; y: number };

const MIN_SCALE = 1;
const MAX_SCALE = 8;

export function MarkdownImageViewer({
  alt,
  image,
  inline = false,
  title,
}: {
  alt: string;
  image: MarkdownResolvedImage;
  inline?: boolean;
  title?: string;
}) {
  const [open, setOpen] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const [transform, setTransform] = useState<Transform>({ scale: 1, x: 0, y: 0 });
  const pointers = useRef(new Map<number, Point>());
  const gesture = useRef<{
    distance: number;
    midpoint: Point;
    transform: Transform;
  } | null>(null);

  const reset = () => setTransform({ scale: 1, x: 0, y: 0 });
  const changeOpen = useCallback((next: boolean) => {
    setOpen(next);
    setLoaded(false);
    setTransform({ scale: 1, x: 0, y: 0 });
    pointers.current.clear();
    gesture.current = null;
  }, []);
  useEffect(() => {
    if (open) return registerBackOverlay(() => changeOpen(false));
  }, [open, changeOpen]);
  const zoom = (factor: number) => {
    setTransform((current) => {
      const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, current.scale * factor));
      return scale === MIN_SCALE ? { scale, x: 0, y: 0 } : { ...current, scale };
    });
  };

  return (
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogTrigger
        type="button"
        className={`markdown-image-trigger group relative max-w-full cursor-zoom-in overflow-hidden rounded-xl text-left ${inline ? "inline-flex align-middle" : "block"}`}
        aria-label={alt ? `Open image: ${alt}` : "Open image viewer"}
        onClick={(event) => event.stopPropagation()}
      >
        <img
          alt={alt}
          decoding="async"
          draggable={false}
          height={image.height}
          loading="lazy"
          referrerPolicy="no-referrer"
          src={image.src}
          title={title}
          width={image.width}
        />
        <span className="pointer-events-none absolute right-2 bottom-2 flex size-8 items-center justify-center rounded-lg bg-black/65 text-white opacity-0 shadow-sm backdrop-blur-sm transition group-hover:opacity-100 group-focus-visible:opacity-100">
          <Maximize2 className="size-4" />
        </span>
      </DialogTrigger>
      <DialogContent
        data-fullscreen="true"
        showCloseButton={false}
        className="inset-0 top-0 left-0 h-dvh w-dvw max-w-none translate-x-0 translate-y-0 gap-0 overflow-hidden rounded-none border-0 bg-black p-0 sm:max-w-none"
        onClick={(event) => event.stopPropagation()}
      >
        <DialogTitle className="sr-only">{alt || title || "Image viewer"}</DialogTitle>
        <DialogDescription className="sr-only">
          Full-resolution image. Use the controls to zoom, reset, or close.
        </DialogDescription>
        <div className="absolute top-[max(0.75rem,var(--safe-area-inset-top))] right-[max(0.75rem,var(--safe-area-inset-right))] left-[max(0.75rem,var(--safe-area-inset-left))] z-20 flex items-center gap-2">
          <div className="flex items-center rounded-xl border border-white/15 bg-black/60 p-1 text-white shadow-lg backdrop-blur-md">
            <ViewerButton
              label="Zoom out"
              onClick={() => zoom(1 / 1.25)}
              disabled={transform.scale <= MIN_SCALE}
            >
              <ZoomOut />
            </ViewerButton>
            <span className="w-14 text-center text-xs tabular-nums">
              {Math.round(transform.scale * 100)}%
            </span>
            <ViewerButton
              label="Zoom in"
              onClick={() => zoom(1.25)}
              disabled={transform.scale >= MAX_SCALE}
            >
              <ZoomIn />
            </ViewerButton>
            <ViewerButton label="Reset zoom" onClick={reset} disabled={transform.scale === 1}>
              <RotateCcw />
            </ViewerButton>
          </div>
          {(title || alt) && (
            <span className="min-w-0 flex-1 truncate px-2 text-center text-sm text-white/75">
              {title || alt}
            </span>
          )}
          <button
            type="button"
            aria-label="Close image viewer"
            className="ml-auto flex size-10 items-center justify-center rounded-xl border border-white/15 bg-black/60 text-xl text-white shadow-lg backdrop-blur-md hover:bg-white/15"
            onClick={() => changeOpen(false)}
          >
            ×
          </button>
        </div>
        <div
          className="relative flex h-full w-full touch-none select-none items-center justify-center overflow-hidden"
          onDoubleClick={() => (transform.scale === 1 ? zoom(2) : reset())}
          onWheel={(event) => {
            event.preventDefault();
            zoom(event.deltaY < 0 ? 1.18 : 1 / 1.18);
          }}
          onPointerDown={(event) => {
            event.currentTarget.setPointerCapture(event.pointerId);
            pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
            gesture.current = gestureState(pointers.current, transform);
          }}
          onPointerMove={(event) => {
            if (!pointers.current.has(event.pointerId)) return;
            const previous = pointers.current.get(event.pointerId);
            pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
            if (pointers.current.size === 1 && previous && transform.scale > 1) {
              setTransform((current) => ({
                ...current,
                x: current.x + event.clientX - previous.x,
                y: current.y + event.clientY - previous.y,
              }));
              return;
            }
            const initial = gesture.current;
            const current = gestureState(pointers.current, transform);
            if (!initial || !current || pointers.current.size < 2) return;
            const scale = Math.min(
              MAX_SCALE,
              Math.max(MIN_SCALE, initial.transform.scale * (current.distance / initial.distance)),
            );
            setTransform({
              scale,
              x: initial.transform.x + current.midpoint.x - initial.midpoint.x,
              y: initial.transform.y + current.midpoint.y - initial.midpoint.y,
            });
          }}
          onPointerUp={(event) => {
            pointers.current.delete(event.pointerId);
            gesture.current = gestureState(pointers.current, transform);
          }}
          onPointerCancel={(event) => {
            pointers.current.delete(event.pointerId);
            gesture.current = gestureState(pointers.current, transform);
          }}
        >
          {!loaded && (
            <div className="absolute inset-0 flex items-center justify-center">
              <img
                aria-hidden="true"
                className="max-h-full max-w-full object-contain"
                draggable={false}
                src={image.src}
              />
            </div>
          )}
          <img
            alt={alt}
            className="max-h-full max-w-full object-contain will-change-transform"
            decoding="async"
            draggable={false}
            onLoad={async (event) => {
              const element = event.currentTarget;
              try {
                await element.decode();
                // A closing dialog can unmount while decoding is in flight.
                if (element.isConnected) setLoaded(true);
              } catch {
                // Keep the preview if the original cannot be decoded.
              }
            }}
            src={open ? image.originalSrc : undefined}
            style={{
              visibility: loaded ? "visible" : "hidden",
              transform: `translate3d(${transform.x}px, ${transform.y}px, 0) scale(${transform.scale})`,
            }}
          />
        </div>
      </DialogContent>
    </Dialog>
  );
}

function ViewerButton({
  children,
  disabled,
  label,
  onClick,
}: {
  children: React.ReactElement<{ className?: string }>;
  disabled?: boolean;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      aria-label={label}
      disabled={disabled}
      onClick={onClick}
      className="flex size-9 items-center justify-center rounded-lg transition hover:bg-white/15 disabled:opacity-35 [&_svg]:size-4"
    >
      {children}
    </button>
  );
}

function gestureState(pointers: ReadonlyMap<number, Point>, transform: Transform) {
  const points = [...pointers.values()];
  if (points.length === 0) return null;
  if (points.length === 1) {
    return { distance: 1, midpoint: points[0], transform };
  }
  const [first, second] = points;
  return {
    distance: Math.max(1, Math.hypot(second.x - first.x, second.y - first.y)),
    midpoint: { x: (first.x + second.x) / 2, y: (first.y + second.y) / 2 },
    transform,
  };
}
