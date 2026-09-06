export function OutlineSkeleton() {
  return (
    <div role="status" aria-label="Loading page content" className="space-y-4 py-4">
      <span className="sr-only">Loading page content…</span>
      <div aria-hidden="true" className="space-y-4 motion-safe:animate-pulse eink:animate-none">
        <div className="h-3 w-4/5 rounded bg-muted" />
        <div className="ml-7 h-3 w-3/5 rounded bg-muted" />
        <div className="ml-7 h-3 w-2/3 rounded bg-muted" />
        <div className="h-3 w-3/4 rounded bg-muted" />
      </div>
    </div>
  );
}
