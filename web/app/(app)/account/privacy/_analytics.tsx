"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Switch } from "@/components/ui/switch";

import { setAnalyticsPreferenceAction } from "./actions";

export function AnalyticsPreference({ optIn }: { optIn: boolean }) {
  const router = useRouter();
  const [pending, startTransition] = useTransition();
  const [checked, setChecked] = useState(optIn);

  const onChange = (next: boolean) => {
    setChecked(next);
    startTransition(async () => {
      try {
        const res = await setAnalyticsPreferenceAction(next);
        if (res.error) {
          setChecked(!next);
          toast.error(res.error);
          return;
        }
        router.refresh();
      } catch {
        setChecked(!next);
        toast.error("Network error. Please try again.");
      }
    });
  };

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
