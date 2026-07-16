import { useQuery } from "@tanstack/react-query";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { listEntities, type Node } from "@/lib/api";
import { queryKeys } from "@/lib/query";

type Props = {
  variant?: "card" | "compact";
};

export function EntitiesCard({ variant = "card" }: Props = {}) {
  const { data: entities = [] } = useQuery<Node[]>({
    queryKey: queryKeys.entities,
    queryFn: () => listEntities(30),
  });

  if (variant === "compact") {
    return (
      <div className="flex flex-col gap-2 border-t border-border/50 pt-3">
        <div className="flex items-center justify-between px-1 text-[11px] font-semibold tracking-wide text-muted-foreground">
          <span>CONCEPTS</span>
          {entities.length > 0 && <span>{entities.length}</span>}
        </div>
        {entities.length === 0 ? (
          <p className="px-1 text-[11px] leading-relaxed text-muted-foreground">
            Concepts appear as your notes are indexed.
          </p>
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
