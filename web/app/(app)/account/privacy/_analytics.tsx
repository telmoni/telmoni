"use client";

import { useOptimistic, useState } from "react";

import { Switch } from "@/components/ui/switch";
import { useToastAction } from "@/lib/use-action";

import { setAnalyticsPreferenceAction } from "./actions";

export function AnalyticsPreference({ optIn }: { optIn: boolean }) {
  const { pending, run } = useToastAction();
  const [saved, setSaved] = useState(optIn);
  // Flipped at once, and back to what is saved when the write does not land.
  const [checked, flip] = useOptimistic(saved);

  const onChange = (next: boolean) =>
    run(
      () => {
        flip(next);
        return setAnalyticsPreferenceAction(next);
      },
      () => setSaved(next),
    );

  return (
    <div className="flex items-start justify-between gap-6">
      <div className="grid gap-1">
        <p className="font-medium">Product analytics</p>
        <p className="text-muted-foreground">
          Off unless you turn it on. While it is on, the console sends the product
          events you trigger &mdash; which steps you complete, never their content
          &mdash; to the analytics provider this deployment has configured, so its
          operator can see where people get stuck. It never includes your API keys
          or anything inside your projects. Either way the platform still writes
          its own operational log, which stays on its servers.
        </p>
      </div>
      <Switch
        checked={checked}
        onCheckedChange={onChange}
        disabled={pending}
        aria-label="Count me in product analytics"
      />
    </div>
  );
}
