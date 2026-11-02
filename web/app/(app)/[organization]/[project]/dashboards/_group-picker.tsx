"use client";

import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useOptimistic, useTransition } from "react";

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ROLLUP_GROUPS, type RollupGroup } from "@/lib/dashboards";

const LABEL: Record<RollupGroup | "none", string> = {
  none: "No grouping",
  model: "By model",
  agent: "By agent",
  provider: "By provider",
  environment: "By environment",
  release: "By release",
};

/** What the table under the charts totals by, held in the page's URL. */
export function GroupPicker({ group }: { group: RollupGroup | null }) {
  const router = useRouter();
  const pathname = usePathname();
  const params = useSearchParams();
  const [shown, show] = useOptimistic<string>(group ?? "none");
  const [pending, start] = useTransition();
  return (
    <Select
      value={shown}
      onValueChange={(value) => {
        const next = new URLSearchParams(params.toString());
        if (value === "none") next.delete("group");
        else next.set("group", value);
        start(() => {
          show(value);
          router.push(`${pathname}?${next.toString()}`, { scroll: false });
        });
      }}
    >
      <SelectTrigger aria-label="Group" aria-busy={pending}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent position="popper" align="end">
        {(["none", ...ROLLUP_GROUPS] as const).map((value) => (
          <SelectItem key={value} value={value}>
            {LABEL[value]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
