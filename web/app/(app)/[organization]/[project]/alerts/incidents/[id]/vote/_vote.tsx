"use client";

import { unstable_rethrow } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Verdict } from "@/lib/types/enums";

import { voteIncidentAction } from "./actions";

const WORDS: { verdict: Verdict; label: string; told: string }[] = [
  { verdict: Verdict.Useful, label: "Useful", told: "Marked useful." },
  { verdict: Verdict.Noise, label: "Noise", told: "Marked as noise." },
];

// The two words a member picks between; theirs, once given, is the filled
// one, and picking the other changes it.
export function VoteButtons({
  projectId,
  incidentId,
  yours,
}: {
  projectId: string;
  incidentId: string;
  yours: Verdict | null;
}) {
  const [error, setError] = useState<string | null>(null);
  const [pending, startTransition] = useTransition();

  const cast = (verdict: Verdict, told: string) => {
    if (pending) return;
    setError(null);
    startTransition(async () => {
      try {
        const res = await voteIncidentAction(projectId, incidentId, verdict);
        if (res.error) {
          setError(res.error);
          toast.error(res.error);
        } else {
          toast.success(told);
        }
      } catch (err) {
        unstable_rethrow(err);
        const msg = "Network error. Please try again.";
        setError(msg);
        toast.error(msg);
      }
    });
  };

  return (
    <div className="grid gap-3">
      <div className="flex flex-wrap gap-3.5">
        {WORDS.map(({ verdict, label, told }) => (
          <Button
            key={verdict}
            type="button"
            variant={yours === verdict ? "default" : "outline"}
            aria-pressed={yours === verdict}
            disabled={pending}
            onClick={() => cast(verdict, told)}
          >
            {label}
          </Button>
        ))}
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
    </div>
  );
}
