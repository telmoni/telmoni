"use client";

import { useId, useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useToastAction } from "@/lib/use-action";
import type { DoorScrub } from "@/lib/server/entities/privacy";

import { setDoorScrubAction, setRetentionAction } from "./actions";

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
      className="flex flex-wrap items-end gap-gutter"
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
      <Button type="submit" variant="filled" disabled={!changed || pending}>
        {pending ? "Saving…" : "Save"}
      </Button>
    </form>
  );
}

/** These switches affect foreign exports, never the developer's SDK policy. */
export function DoorScrubForm({ projectId, policy, disabled }: {
  projectId: string;
  policy: DoorScrub;
  disabled: boolean;
}) {
  const [secrets, setSecrets] = useState(policy.door_scrub_secrets);
  const [pii, setPii] = useState(policy.door_scrub_pii);
  const { pending, run } = useToastAction();
  return (
    <form className="grid gap-gutter" onSubmit={(event) => {
      event.preventDefault();
      if (disabled || pending) return;
      run(() => setDoorScrubAction(projectId, { door_scrub_secrets: secrets, door_scrub_pii: pii }),
        () => toast.success("Door scrubbers updated."));
    }}>
      <label className="flex items-center gap-gutter">
        <Checkbox checked={secrets} disabled={disabled || pending} onCheckedChange={(checked) => setSecrets(checked === true)} />
        Scrub secrets at the door
      </label>
      <label className="flex items-center gap-gutter">
        <Checkbox checked={pii} disabled={disabled || pending} onCheckedChange={(checked) => setPii(checked === true)} />
        Scrub personal data at the door
      </label>
      {!disabled && <Button className="justify-self-start" type="submit" variant="filled" disabled={pending || (secrets === policy.door_scrub_secrets && pii === policy.door_scrub_pii)}>
        {pending ? "Saving…" : "Save"}
      </Button>}
    </form>
  );
}
