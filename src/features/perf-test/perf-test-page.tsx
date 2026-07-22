import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { MarkdownRenderer } from "@/features/markdown";
import type { MarkdownRenderContext } from "@/features/markdown/types";

// Bottom-up scroll-smoothness bisection. Each level adds ONE layer of complexity
// on top of a known-smooth bare scroller, so we can measure jerkNorm per level and
// see exactly which layer introduces the lag. Reach a level via `#perftest=<n>`.
//
//   0  plain <div> rows (bare scroller baseline — should match ~0.39)
//   1  React component rows (React reconcile, NO virtualization / no scroll re-render)
//   2  tanstack-virtual, simple rows (adds re-render on every scroll event)
//   3  tanstack-virtual + measureElement (adds per-row ResizeObserver churn)
//   4  tanstack-virtual + measureElement + MarkdownRenderer per row (adds markdown + heavy DOM/a11y)

const N = 2000;

function line(i: number): string {
  return `Row ${i} — sample content line for the perf bisection, long enough to wrap on a phone screen and exercise layout and paint on scroll.`;
}
function markdown(i: number): string {
  return `**Row ${i}** — sample *markdown* with a [link](notes-page:x), \`inline code\`, and enough text to wrap onto a couple of lines. Item number ${i}.`;
}

const MD_CTX: MarkdownRenderContext = {
  kind: "note",
  presentation: "reading",
  pageUuid: "perf-test",
  blockUuid: "perf-test",
};

function Scroll({ children }: { children: React.ReactNode }) {
  return (
    <div
      data-workspace-scroll
      className="h-screen overflow-y-auto bg-background px-4 text-sm text-foreground"
    >
      {children}
    </div>
  );
}

function Level0() {
  return (
    <Scroll>
      {Array.from({ length: N }, (_, i) => (
        <div key={i} className="border-b border-border/40 py-3">
          {line(i)}
        </div>
      ))}
    </Scroll>
  );
}

function Row({ i }: { i: number }) {
  return <div className="border-b border-border/40 py-3">{line(i)}</div>;
}
function Level1() {
  return (
    <Scroll>
      {Array.from({ length: N }, (_, i) => (
        <Row key={i} i={i} />
      ))}
    </Scroll>
  );
}

function VirtualList({
  measure,
  renderRow,
}: {
  measure: boolean;
  renderRow: (i: number) => React.ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const v = useVirtualizer({
    count: N,
    estimateSize: () => 72,
    getScrollElement: () => ref.current,
    overscan: 8,
  });
  return (
    <div
      ref={ref}
      data-workspace-scroll
      className="h-screen overflow-y-auto bg-background px-4 text-sm text-foreground"
    >
      <div style={{ height: v.getTotalSize(), position: "relative", contain: "layout style" }}>
        {v.getVirtualItems().map((item) => (
          <div
            key={item.key}
            data-index={item.index}
            ref={measure ? v.measureElement : undefined}
            className="border-b border-border/40 py-3"
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              width: "100%",
              transform: `translateY(${item.start}px)`,
            }}
          >
            {renderRow(item.index)}
          </div>
        ))}
      </div>
    </div>
  );
}

export function PerfTestPage({ level }: { level: number }) {
  if (level === 0) return <Level0 />;
  if (level === 1) return <Level1 />;
  if (level === 2) return <VirtualList measure={false} renderRow={(i) => line(i)} />;
  if (level === 3) return <VirtualList measure renderRow={(i) => line(i)} />;
  return (
    <VirtualList
      measure
      renderRow={(i) => (
        <MarkdownRenderer context={MD_CTX} markdown={markdown(i)} mode="compact_flow" />
      )}
    />
  );
}
