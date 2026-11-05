// @vitest-environment jsdom
import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

// Recharts measures a box jsdom has none of: the charts draw their frames alone.
vi.mock("@/lib/in-browser", () => ({ useInBrowser: () => false }));

import { DashboardCharts } from "./_charts";

const point = (value: string, spend: number, others = false) => ({
  at: "2026-11-02T00:00:00Z",
  value,
  others,
  spans: 3,
  failed: 0,
  runs: 1,
  failed_runs: 0,
  input_tokens: 10,
  output_tokens: 5,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  spend,
  run_duration_ms: { p50: 1, p95: 2, p99: 3 },
});

function draw(group: "user" | "model", days: number) {
  render(
    <DashboardCharts
      organization="acme"
      slug="web"
      charts={[{ measure: "spend", group }, { measure: "runs" }]}
      days={90}
      bars={[]}
      firstToken={[]}
      hourly={false}
      firstTokenHourly={false}
      splits={{
        [`spend:${group}`]: {
          kind: "ok",
          days,
          points: [point("u-1", 4), point("", 1), point("", 2, true)],
        },
      }}
    />,
  );
}

describe("DashboardCharts", () => {
  it("totals a split under its chart, each user opening their runs, and says how far the spans reach", () => {
    draw("user", 30);
    const split = within(screen.getByRole("region", { name: "Spend by user" }));
    expect(split.getByRole("link", { name: "u-1" })).toHaveAttribute("href", "/acme/web/traces?user=u-1&days=30");
    expect(split.getByText("Other values")).toBeInTheDocument();
    expect(split.getByText("None")).toBeInTheDocument();
    expect(split.getAllByText(/^\$[\d.]+$/).map((cell) => cell.textContent)).toEqual(["$4.00", "$2.00", "$1.00"]);
    expect(split.getByText(/^The last 30 days/)).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Runs" })).toBeInTheDocument();
  });

  it("links no model, and says nothing of retention for the totals", () => {
    draw("model", 90);
    const split = within(screen.getByRole("region", { name: "Spend by model" }));
    expect(split.queryByRole("link")).toBeNull();
    expect(split.queryByText(/^The last/)).toBeNull();
  });

  it("names a split it could not read, and draws the rest", () => {
    render(
      <DashboardCharts
        organization="acme"
        slug="web"
        charts={[{ measure: "spend", group: "tag", tag: "team" }]}
        days={7}
        bars={[]}
        firstToken={[]}
        hourly={false}
        firstTokenHourly={false}
        splits={{ "spend:tag:team": { kind: "unread" } }}
      />,
    );
    const split = within(screen.getByRole("region", { name: "Spend by tag team" }));
    expect(split.getByText(/could not be read/)).toBeInTheDocument();
  });
});
