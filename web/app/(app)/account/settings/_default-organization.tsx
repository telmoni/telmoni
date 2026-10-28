"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useToastAction } from "@/lib/use-action";

import { setDefaultOrganizationAction } from "./actions";

export interface DefaultOrganizationChoice {
  organizationId: string;
  label: string;
}

// The organization a sign-in opens in. `current` is the one auth answers now —
// chosen here, or
// picked for whoever has not chosen or has left their choice.
export function DefaultOrganization({
  organizations,
  current,
}: {
  organizations: DefaultOrganizationChoice[];
  current: string;
}) {
  const [chosen, setChosen] = useState(current);
  const { pending, run } = useToastAction();

  const onSave = () => run(() => setDefaultOrganizationAction(chosen), "Default organization saved.");

  return (
    <div className="grid gap-3">
      <Select value={chosen} onValueChange={setChosen} disabled={pending}>
        <SelectTrigger aria-label="Default organization" className="w-full sm:w-72">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {organizations.map((o) => (
            <SelectItem key={o.organizationId} value={o.organizationId}>
              {o.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <div>
        <Button variant="outline" onClick={onSave} disabled={pending || chosen === current}>
          {pending ? "Saving…" : "Save"}
        </Button>
      </div>
    </div>
  );
}
