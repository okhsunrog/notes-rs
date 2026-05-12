import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { listEntities, type Node } from "@/lib/api";

type Props = {
  variant?: "card" | "compact";
};

export function EntitiesCard({ variant = "card" }: Props = {}) {
  const [entities, setEntities] = useState<Node[]>([]);

  const refresh = useCallback(async () => {
    try {
      setEntities(await listEntities(30));
    } catch {
      /* ignore */
    }
  }, []);

  useEffect(() => {
    refresh();
    const unlistenPromise = listen("entities:changed", () => {
      void refresh();
    });
    return () => {
      void unlistenPromise.then((un) => un());
    };
  }, [refresh]);

  if (variant === "compact") {
    return (
      <div className="flex flex-col gap-2">
        <div className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          Entities
        </div>
        {entities.length === 0 ? (
          <p className="text-xs italic text-muted-foreground">no entities yet</p>
        ) : (
          <div className="flex flex-wrap gap-1">
            {entities.map((e) => (
              <Badge key={e.id} variant="secondary" title={e.content} className="text-xs">
                {e.title}
              </Badge>
            ))}
          </div>
        )}
      </div>
    );
  }

  return (
    <Card>
      <CardHeader>
        <CardTitle>Entities</CardTitle>
        <CardDescription>
          Extracted from your notes by the background worker. Top by mention count.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {entities.length === 0 ? (
          <p className="text-sm italic text-muted-foreground">
            no entities yet — create a few notes and wait a moment.
          </p>
        ) : (
          <div className="flex flex-wrap gap-2">
            {entities.map((e) => (
              <Badge key={e.id} variant="secondary" title={e.content}>
                {e.title}
              </Badge>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
