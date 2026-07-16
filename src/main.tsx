import React from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider } from "next-themes";
import App from "./App";
import { AppearanceProvider } from "@/app/appearance";
import { ErrorBoundary } from "@/app/error-boundary";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <AppearanceProvider>
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </AppearanceProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
