import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";

export type BlockEditHandle = {
  /** Replace text in `[start, end)` with `replacement`, then place caret at
   * `caret` (defaults to end of replacement). Triggers `onChange` so the host
   * sees the new value/caret. */
  replaceRange: (start: number, end: number, replacement: string, caret?: number) => void;
  focus: () => void;
  /** Current caret position (selectionStart). */
  getCaret: () => number;
};

type Props = {
  initial: string;
  onChange: (value: string, caret: number) => void;
  onBlur: () => void;
  onKeyDown?: (e: React.KeyboardEvent<HTMLTextAreaElement>) => void;
  autoFocus?: boolean;
};

export const BlockEdit = forwardRef<BlockEditHandle, Props>(function BlockEdit(
  { initial, onChange, onBlur, onKeyDown, autoFocus },
  ref,
) {
  const taRef = useRef<HTMLTextAreaElement>(null);

  const resize = () => {
    const el = taRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${el.scrollHeight}px`;
  };

  useImperativeHandle(ref, () => ({
    replaceRange(start, end, replacement, caret) {
      const el = taRef.current;
      if (!el) return;
      const value = el.value;
      const next = value.slice(0, start) + replacement + value.slice(end);
      el.value = next;
      const c = caret ?? start + replacement.length;
      el.setSelectionRange(c, c);
      resize();
      onChange(next, c);
    },
    focus() {
      taRef.current?.focus();
    },
    getCaret() {
      return taRef.current?.selectionStart ?? 0;
    },
  }));

  useEffect(() => {
    resize();
    if (autoFocus && taRef.current) {
      const el = taRef.current;
      el.focus();
      const len = el.value.length;
      el.setSelectionRange(len, len);
    }
  }, [autoFocus]);

  return (
    <textarea
      ref={taRef}
      defaultValue={initial}
      onInput={(e) => {
        resize();
        const el = e.currentTarget;
        onChange(el.value, el.selectionStart);
      }}
      onKeyUp={(e) => {
        // Caret can move via arrow keys without firing onInput. Notify host
        // so trigger detection picks it up.
        const el = e.currentTarget;
        if (
          e.key === "ArrowLeft" ||
          e.key === "ArrowRight" ||
          e.key === "Home" ||
          e.key === "End"
        ) {
          onChange(el.value, el.selectionStart);
        }
      }}
      onClick={(e) => {
        const el = e.currentTarget;
        onChange(el.value, el.selectionStart);
      }}
      onBlur={onBlur}
      onKeyDown={onKeyDown}
      rows={1}
      className="min-h-[1.5rem] w-full resize-none border-0 bg-transparent p-0 text-sm leading-relaxed text-foreground outline-none focus-visible:ring-0"
    />
  );
});
