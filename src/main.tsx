import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider } from "next-themes";
import App from "./App";
import { AppearanceProvider } from "@/app/appearance";
import { ConfirmationProvider } from "@/app/confirmation";
import { ErrorBoundary } from "@/app/error-boundary";
import { disableViewportZoom } from "@/app/viewport-zoom";
import { PageSessionProvider } from "@/features/pages/page-session";
import { PerfTestPage } from "@/features/perf-test/perf-test-page";
import { Toaster } from "@/components/ui/sonner";
import "./index.css";

import { createAppQueryClient, listenForDomainEvents } from "@/lib/query";

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

// Scroll-smoothness bisection harness — reach a level via `#perftest=<n>` (no hash = normal app).
const perfTest = new URLSearchParams(location.hash.replace(/^#/, "")).get("perftest");
if (perfTest !== null) {
  disableViewportZoom();
  root.render(
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <PerfTestPage level={Number(perfTest) || 0} />
    </ThemeProvider>,
  );
} else {
  const queryClient = createAppQueryClient();
  void listenForDomainEvents(queryClient);
  disableViewportZoom();

  root.render(
    <React.StrictMode>
      <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
        <QueryClientProvider client={queryClient}>
          <AppearanceProvider>
            <ConfirmationProvider>
              <ErrorBoundary>
                <PageSessionProvider>
                  <App />
                </PageSessionProvider>
              </ErrorBoundary>
            </ConfirmationProvider>
          </AppearanceProvider>
        </QueryClientProvider>
        <Toaster
          position="bottom-right"
          mobileOffset={{
            right: "calc(1rem + var(--safe-area-inset-right))",
            bottom: "calc(1rem + var(--safe-area-inset-bottom))",
            left: "calc(1rem + var(--safe-area-inset-left))",
          }}
        />
      </ThemeProvider>
    </React.StrictMode>,
  );
}
