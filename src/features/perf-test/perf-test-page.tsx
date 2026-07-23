import { useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { MarkdownRenderer } from "@/features/markdown";
import type { MarkdownRenderContext } from "@/features/markdown/types";

// Bottom-up scroll-smoothness bisection. Each level adds ONE layer of complexity on
// top of a known-smooth bare scroller. Rows are cards with VARIED text (not identical)
// and a slightly larger font, so a repeating-pattern optical illusion can't be mistaken
// for real scroll lag. Reach a level via `#perftest=<n>`.
//
//   0 plain card rows            1 React component rows        2 virtualized simple
//   3 + measureElement           4 + MarkdownRenderer/row      5 markdown, no virtualization
//   6 virtualized + cached HTML

const N = 2000;

function prng(seed: number): () => number {
  let s = (seed * 2654435761) >>> 0;
  return () => {
    s = (s * 1103515245 + 12345) >>> 0;
    return s / 0xffffffff;
  };
}
const WORDS =
  "the quick brown fox jumps over a lazy dog system render scroll paint layout virtual compositor frame budget markdown content sample phone screen wrap deceleration fling momentum smooth rough cache mount react node tree raster thread main idle busy overscan measure element reconcile parse token style card list".split(
    " ",
  );
function words(r: () => number, n: number): string {
  return Array.from({ length: n }, () => WORDS[Math.floor(r() * WORDS.length)]).join(" ");
}
function line(i: number): string {
  const r = prng(i + 1);
  return `${i}. ` + words(r, 6 + Math.floor(r() * 28));
}
function markdown(i: number): string {
  const r = prng(i + 100);
  const n = 6 + Math.floor(r() * 22);
  const parts: string[] = [`**Item ${i}**`];
  for (let k = 0; k < n; k++) {
    const w = WORDS[Math.floor(r() * WORDS.length)];
    const roll = r();
    if (roll < 0.08) parts.push(`*${w}*`);
    else if (roll < 0.14) parts.push(`\`${w}\``);
    else if (roll < 0.18) parts.push(`[${w}](notes-page:x)`);
    else parts.push(w);
  }
  return parts.join(" ");
}

const MD_CTX: MarkdownRenderContext = {
  kind: "note",
  presentation: "reading",
  pageUuid: "perf-test",
  blockUuid: "perf-test",
};

const CARD =
  "rounded-2xl border border-border/60 bg-card p-4 text-base leading-relaxed text-card-foreground shadow-sm";

function Scroll({ children }: { children: React.ReactNode }) {
  return (
    <div data-workspace-scroll className="h-screen space-y-3 overflow-y-auto bg-background p-3">
      {children}
    </div>
  );
}

function Level0() {
  return (
    <Scroll>
      {Array.from({ length: N }, (_, i) => (
        <div key={i} className={CARD}>
          {line(i)}
        </div>
      ))}
    </Scroll>
  );
}

function Row({ i }: { i: number }) {
  return <div className={CARD}>{line(i)}</div>;
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
    estimateSize: () => 132,
    getScrollElement: () => ref.current,
    overscan: 8,
  });
  return (
    <div ref={ref} data-workspace-scroll className="h-screen overflow-y-auto bg-background p-3">
      <div style={{ height: v.getTotalSize(), position: "relative", contain: "layout style" }}>
        {v.getVirtualItems().map((item) => (
          <div
            key={item.key}
            data-index={item.index}
            ref={measure ? v.measureElement : undefined}
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              width: "100%",
              transform: `translateY(${item.start}px)`,
            }}
          >
            <div className={`${CARD} mb-3`}>{renderRow(item.index)}</div>
          </div>
        ))}
      </div>
    </div>
  );
}

function mdToHtml(md: string): string {
  return md
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>")
    .replace(/`(.+?)`/g, "<code>$1</code>")
    .replace(/\[(.+?)\]\((.+?)\)/g, "<a>$1</a>")
    .replace(/\*(.+?)\*/g, "<em>$1</em>");
}
const HTML_CACHE = Array.from({ length: N }, (_, i) => mdToHtml(markdown(i)));

function Level5NonVirtualMarkdown() {
  return (
    <Scroll>
      {Array.from({ length: 300 }, (_, i) => (
        <div key={i} className={CARD}>
          <MarkdownRenderer context={MD_CTX} markdown={markdown(i)} mode="compact_flow" />
        </div>
      ))}
    </Scroll>
  );
}

export function PerfTestPage({ level }: { level: number }) {
  if (level === 0) return <Level0 />;
  if (level === 1) return <Level1 />;
  if (level === 2) return <VirtualList measure={false} renderRow={(i) => line(i)} />;
  if (level === 3) return <VirtualList measure renderRow={(i) => line(i)} />;
  if (level === 4) {
    return (
      <VirtualList
        measure
        renderRow={(i) => (
          <MarkdownRenderer context={MD_CTX} markdown={markdown(i)} mode="compact_flow" />
        )}
      />
    );
  }
  if (level === 5) return <Level5NonVirtualMarkdown />;
  return (
    <VirtualList
      measure
      renderRow={(i) => <div dangerouslySetInnerHTML={{ __html: HTML_CACHE[i] }} />}
    />
  );
}
