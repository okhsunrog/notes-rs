import { useEffect, useRef } from "react";

type Props = {
  initial: string;
  onChange: (value: string) => void;
  onBlur: () => void;
  onKeyDown?: (e: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  autoFocus?: boolean;
};

/** Auto-growing textarea for editing a single block's markdown. */
export function BlockEdit({ initial, onChange, onBlur, onKeyDown, autoFocus }: Props) {
  const ref = useRef<HTMLTextAreaElement>(null);

  const resize = () => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  };

  useEffect(() => {
    resize();
    if (autoFocus && ref.current) {
      const el = ref.current;
      el.focus();
      // Place caret at end for a more natural enter-edit experience.
      const len = el.value.length;
      el.setSelectionRange(len, len);
    }
  }, [autoFocus]);

  return (
    <textarea
      ref={ref}
      defaultValue={initial}
      onInput={(e) => {
        resize();
        onChange(e.currentTarget.value);
      }}
      onBlur={onBlur}
      onKeyDown={onKeyDown}
      rows={1}
      className="min-h-[1.5rem] w-full resize-none border-0 bg-transparent p-0 text-sm leading-relaxed text-foreground outline-none focus-visible:ring-0"
    />
  );
}
