import { useEffect, useState } from "react";

const COMPACT_WORKSPACE_WIDTH = 1000;

export function useCompactLayout() {
  const [compact, setCompact] = useState(() => window.innerWidth < COMPACT_WORKSPACE_WIDTH);
  useEffect(() => {
    const resize = () => setCompact(window.innerWidth < COMPACT_WORKSPACE_WIDTH);
    window.addEventListener("resize", resize);
    return () => window.removeEventListener("resize", resize);
  }, []);
  return compact;
}
