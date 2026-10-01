import type { ReactNode } from "react";

import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export function Rows({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <Card className={cn("gap-0 divide-y divide-border py-0 text-sm", className)}>
      {children}
    </Card>
  );
}

export function Row({
  label,
  hint,
  children,
  danger,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children?: ReactNode;
  danger?: boolean;
}) {
  return (
    <div className="flex min-h-14 items-center justify-between gap-6 py-3">
      <div className="grid min-w-0 gap-0.5">
        <span className={cn("font-medium", danger && "text-destructive")}>{label}</span>
        {hint ? <span className="text-muted-foreground">{hint}</span> : null}
      </div>
      {children !== undefined && (
        <div className="flex min-w-0 items-center justify-end gap-2 text-right">
          {children}
        </div>
      )}
    </div>
  );
}
