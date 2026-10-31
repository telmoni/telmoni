"use client";

import { useId, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useToastAction } from "@/lib/use-action";

import { setRetentionAction } from "./actions";

/** An owner's or admin's choice of how many days the project keeps its spans
 *  and their content, up to the longest sold; the nightly purge holds it. */
export function RetentionForm({
  projectId,
  days,
  max,
}: {
  projectId: string;
  days: number;
  max: number;
}) {
  const id = useId();
  const [value, setValue] = useState(String(days));
  const { pending, run } = useToastAction();
  const chosen = Number(value);
  const valid = Number.isInteger(chosen) && chosen >= 1 && chosen <= max;
  const changed = valid && chosen !== days;

  return (
    <form
      className="flex flex-wrap items-end gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (!changed || pending) return;
        run(
          () => setRetentionAction(projectId, chosen),
          () => toast.success(`Spans are kept ${chosen} days from tonight's purge.`),
        );
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor={id}>Keep spans for</Label>
        <div className="flex items-center gap-2">
          <Input
            id={id}
            type="number"
            inputMode="numeric"
            min={1}
            max={max}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            disabled={pending}
            className="w-24"
          />
          <span className="text-sm text-muted-foreground">days, up to {max}</span>
        </div>
      </div>
      <Button type="submit" disabled={!changed || pending}>
        {pending ? "Saving…" : "Save"}
      </Button>
    </form>
  );
}
