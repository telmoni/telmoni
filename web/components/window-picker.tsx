"use client";

import { CalendarIcon } from "lucide-react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useOptimistic, useTransition } from "react";

import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { windowLabel, windowsOffered } from "@/lib/list-query";

// The window a list reads: a pick
// is the page's URL with the new window in it, everything else the URL holds
// — the sort, the filters, a saved view — kept, and the list back on its
// first page, which the new window may not reach past.
//
// Never disabled while the new window loads: the trigger holds the focus the
// pick left on it, and a disabled control drops it to the page. A second pick
// in the meantime is only another navigation. The window picked shows at once
// and gives way to the server's when the list lands; until then the trigger
// is marked pending, which dims the list it heads (`group/list`).
export function WindowPicker({ days }: { days: number }) {
  const router = useRouter();
  const pathname = usePathname();
  const params = useSearchParams();
  const [shown, show] = useOptimistic(days);
  const [pending, start] = useTransition();
  return (
    <Select
      value={String(shown)}
      onValueChange={(value) => {
        const next = new URLSearchParams(params.toString());
        next.set("days", value);
        next.delete("page");
        start(() => {
          show(Number(value));
          router.push(`${pathname}?${next.toString()}`, { scroll: false });
        });
      }}
    >
      <SelectTrigger
        aria-label="Window"
        aria-busy={pending}
        data-pending={pending ? "" : undefined}
      >
        <CalendarIcon />
        <SelectValue />
      </SelectTrigger>
      <SelectContent position="popper" align="end">
        {windowsOffered(days).map((w) => (
          <SelectItem key={w} value={String(w)}>
            {windowLabel(w)}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}
