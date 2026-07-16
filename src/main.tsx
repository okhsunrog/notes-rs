import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { ThemeProvider } from "next-themes";
import App from "./App";
import { AppearanceProvider } from "@/app/appearance";
import { ErrorBoundary } from "@/app/error-boundary";
import { disableViewportZoom } from "@/app/viewport-zoom";
import "./index.css";

import { createAppQueryClient, listenForDomainEvents } from "@/lib/query";

const queryClient = createAppQueryClient();
void listenForDomainEvents(queryClient);
disableViewportZoom();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <QueryClientProvider client={queryClient}>
        <AppearanceProvider>
          <ErrorBoundary>
            <App />
          </ErrorBoundary>
        </AppearanceProvider>
      </QueryClientProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
