"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

import { setDefaultOrganizationAction } from "./actions";

export interface DefaultOrganizationChoice {
  organizationId: string;
  label: string;
}

// The organization a sign-in opens in, as Vercel's default team is the one
// shown at sign-in. `current` is the one auth answers now — chosen here, or
// picked for whoever has not chosen or has left their choice.
export function DefaultOrganization({
  organizations,
  current,
}: {
  organizations: DefaultOrganizationChoice[];
  current: string;
}) {
  const router = useRouter();
  const [chosen, setChosen] = useState(current);
  const [pending, startTransition] = useTransition();

  const onSave = () => {
    startTransition(async () => {
      try {
        const res = await setDefaultOrganizationAction(chosen);
        if (res.error) {
          toast.error(res.error);
          return;
        }
        toast.success("Default organization saved.");
        // The page names the default auth answers, which this has just moved.
        router.refresh();
      } catch {
        toast.error("Network error. Please try again.");
      }
    });
  };

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
