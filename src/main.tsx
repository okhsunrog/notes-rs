import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider } from "next-themes";
import App from "./App";
import { AppearanceProvider } from "@/app/appearance";
import { ConfirmationProvider } from "@/app/confirmation";
import { ErrorBoundary } from "@/app/error-boundary";
import { installTextFocusSuppression } from "@/app/ink-suppression";
import { registerLifecycleFlush } from "@/app/lifecycle-flush";
import { disableViewportZoom } from "@/app/viewport-zoom";
import { PageSessionProvider, PageSessionRegistry } from "@/features/pages/page-session";
import { InputCapabilitiesProvider } from "@/features/handwriting/input-capabilities";
import { Toaster } from "@/components/ui/sonner";
import "./index.css";

import { createAppQueryClient, listenForDomainEvents } from "@/lib/query";

const root = ReactDOM.createRoot(document.getElementById("root") as HTMLElement);

const queryClient = createAppQueryClient();
void listenForDomainEvents(queryClient);
disableViewportZoom();
// The firmware pen has to be down before the soft keyboard is up, and only the page knows a DOM
// field took the caret. Registered for the whole document, not per editor.
installTextFocusSuppression();
// Owned here rather than by the provider so the process-level drain can reach
// unsaved drafts without a React tree.
const pageSessions = new PageSessionRegistry();
registerLifecycleFlush(pageSessions);

root.render(
  <React.StrictMode>
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <QueryClientProvider client={queryClient}>
        <AppearanceProvider>
          <ConfirmationProvider>
            <ErrorBoundary>
              <PageSessionProvider registry={pageSessions}>
                <InputCapabilitiesProvider>
                  <App />
                </InputCapabilitiesProvider>
              </PageSessionProvider>
            </ErrorBoundary>
          </ConfirmationProvider>
          {/* Inside the appearance provider: the toaster's duration and icons follow the
              resolved display profile. */}
          <Toaster
            position="bottom-right"
            mobileOffset={{
              right: "calc(1rem + var(--safe-area-inset-right))",
              bottom: "calc(1rem + var(--safe-area-inset-bottom))",
              left: "calc(1rem + var(--safe-area-inset-left))",
            }}
          />
        </AppearanceProvider>
      </QueryClientProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
