import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

type State = { error: Error | null };

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("unhandled React error", error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <main className="app-shell flex h-full items-center justify-center p-6 text-foreground">
        <section className="max-w-lg rounded-lg border border-destructive/40 bg-card p-6">
          <h1 className="font-semibold text-destructive">The interface encountered an error</h1>
          <p className="mt-2 text-sm break-words text-muted-foreground">
            {this.state.error.message}
          </p>
          <div className="mt-4 flex gap-2">
            <Button onClick={() => this.setState({ error: null })}>Try again</Button>
            <Button variant="outline" onClick={() => window.location.reload()}>
              Reload UI
            </Button>
          </div>
        </section>
      </main>
    );
  }
}
