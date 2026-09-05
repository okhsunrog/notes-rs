import { lazy, Suspense } from "react";
import { useHandwritingSession } from "./handwriting-session";

const Sheet = lazy(() =>
  import("./handwriting-sheet").then((module) => ({ default: module.HandwritingSheet })),
);

export function HandwritingHost() {
  const open = useHandwritingSession((state) => state.open);
  return open ? (
    <Suspense fallback={null}>
      <Sheet />
    </Suspense>
  ) : null;
}
