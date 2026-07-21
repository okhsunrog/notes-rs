import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeAll, describe, expect, it } from "vite-plus/test";
import { PageSessionProvider, PageSessionRegistry } from "@/features/pages/page-session";
import type { Block } from "@/lib/api";
import { BlockNode } from "./block-node";
import { OutlinerProvider } from "./outliner-store";

const block: Block = {
  uuid: "019c8d1a-4ab1-7f31-8f00-f594337c3ca5",
  pageUuid: "019c8d1a-4ab1-7f31-8f00-f594337c3ca6",
  parentUuid: null,
  orderKey: "a0",
  style: { kind: "paragraph" },
  markdown: "Measured content",
  markdownRevision: "test-markdown-revision",
  createdAt: 0,
  updatedAt: 0,
};

beforeAll(() => {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: () => null,
      removeItem: () => undefined,
      setItem: () => undefined,
    },
  });
});

describe("BlockNode virtualization contract", () => {
  it("marks the measured element with its virtual row index", () => {
    const queryClient = new QueryClient();
    const html = renderToStaticMarkup(
      <QueryClientProvider client={queryClient}>
        <PageSessionProvider registry={new PageSessionRegistry()}>
          <OutlinerProvider
            blocks={[block]}
            layout="outline"
            onOpenMarkdownLink={() => undefined}
            readOnly
          >
            <BlockNode block={block} depth={0} ordinal={1} virtualIndex={7} />
          </OutlinerProvider>
        </PageSessionProvider>
      </QueryClientProvider>,
    );

    expect(html).toContain('data-index="7"');
  });
});
