"use client";

import { ChartColumnIcon } from "lucide-react";
import { usePathname, useRouter, useSearchParams } from "next/navigation";
import { useTransition } from "react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  DASHBOARD_CHARTS_MAX,
  SPEND_GROUPS,
  chartWord,
  type Chart,
  type SpendGroup,
} from "@/lib/dashboards";
import { ChartMeasure, MetricsGroup } from "@/lib/types/enums";

import { GROUP_LABEL, MEASURE_LABEL } from "./_labels";

/** The charts the page draws, held in its URL's `chart` values: a measure
 *  ticked on or off, or spend split by a group or a tag's key; the last
 *  chart stays, and none past the server's twelve is offered. */
export function ChartsPicker({ charts, tagKeys }: { charts: readonly Chart[]; tagKeys: readonly string[] }) {
  const router = useRouter();
  const pathname = usePathname();
  const params = useSearchParams();
  const [pending, start] = useTransition();
  const words = charts.map(chartWord);
  const full = charts.length >= DASHBOARD_CHARTS_MAX;

  const toggle = (chart: Chart) => {
    const word = chartWord(chart);
    const next = words.includes(word) ? words.filter((w) => w !== word) : [...words, word];
    const query = new URLSearchParams(params.toString());
    query.delete("chart");
    for (const each of next) query.append("chart", each);
    start(() => router.push(`${pathname}?${query.toString()}`, { scroll: false }));
  };
  const item = (chart: Chart, label: string) => {
    const on = words.includes(chartWord(chart));
    return (
      <DropdownMenuCheckboxItem
        key={chartWord(chart)}
        checked={on}
        disabled={on ? charts.length === 1 : full}
        onSelect={(event) => {
          event.preventDefault();
          toggle(chart);
        }}
      >
        {label}
      </DropdownMenuCheckboxItem>
    );
  };
  // Trimmed as the URL's are read, so a key shows ticked once drawn.
  const keys = [...new Set(tagKeys.map((key) => key.trim()).filter(Boolean))];
  const groups = SPEND_GROUPS.filter((group): group is Exclude<SpendGroup, "tag"> => group !== MetricsGroup.Tag);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="outline" aria-busy={pending}>
          <ChartColumnIcon aria-hidden="true" />
          Charts
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        <DropdownMenuLabel>Charts</DropdownMenuLabel>
        {Object.values(ChartMeasure).map((measure) => item({ measure }, MEASURE_LABEL[measure]))}
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Spend by</DropdownMenuLabel>
        {groups.map((group) => item({ measure: ChartMeasure.Spend, group }, GROUP_LABEL[group]))}
        <DropdownMenuSub>
          <DropdownMenuSubTrigger disabled={keys.length === 0}>Tag</DropdownMenuSubTrigger>
          <DropdownMenuSubContent>
            {keys.map((tag) => item({ measure: ChartMeasure.Spend, group: MetricsGroup.Tag, tag }, tag))}
          </DropdownMenuSubContent>
        </DropdownMenuSub>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
