// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

// Recharts measures a box jsdom has none of: the charts draw their frames alone.
vi.mock("@/lib/in-browser", () => ({ useInBrowser: () => false }));

import { StoreProvider } from "@/lib/store/provider";

import { DashboardCharts } from "./_charts";

// The charts read the signed-in user to key their saved layout.
const withStore = (ui: React.ReactElement) => (
  <StoreProvider user={null} flags={{}} projects={[]} givenSeats={[]}>
    {ui}
  </StoreProvider>
);

const bar = (at: string, spend: number, failed: number) => ({
  at,
  spend,
  ok: 3,
  failed,
  p50: 1_000,
  p95: 2_000,
  p99: 3_000,
});

describe("DashboardCharts", () => {
  it("draws speed beside first token, each chart said in words", () => {
    render(
      withStore(
        <DashboardCharts
          bars={[
            bar("2026-11-01T00:00:00Z", 1.5, 1),
            bar("2026-11-02T00:00:00Z", 2.5, 0),
          ]}
          firstToken={[
            {
              at: "2026-11-02T00:00:00Z",
              firstTokenP50: 300,
              firstTokenP95: 900,
            },
          ]}
          hourly={false}
          firstTokenHourly={false}
          histogram={{
            days: 7,
            bucket_seconds: 86400,
            utc_offset: 0,
            score: null,
            bars: [],
          }}
          tokenBars={[
            {
              at: "2026-11-01T00:00:00Z",
              tokens_by_model: {
                m: {
                  input_tokens: 200,
                  output_tokens: 20,
                  cache_read_input_tokens: 40,
                  cache_write_input_tokens: 10,
                  reasoning_output_tokens: 5,
                },
              },
            },
          ]}
        />,
      ),
    );
    expect(
      screen
        .getAllByRole("region")
        .map((region) => region.getAttribute("aria-labelledby")),
    ).toHaveLength(6);
    expect(
      screen
        .getAllByRole("heading", { level: 2 })
        .map((heading) => heading.textContent),
    ).toEqual([
      "Spend",
      "Tokens",
      "Traces",
      "Latency",
      "Time to first token",
      "Output speed",
    ]);
    expect(screen.getByText("$4.00 over the window.")).toBeInTheDocument();
    expect(
      screen.getByText(
        /150 input, 40 cache read, 10 cache write, 15 output and 5 reasoning tokens/,
      ),
    ).toBeInTheDocument();
    // One model leaves nothing to pick between.
    expect(screen.queryByRole("combobox", { name: "Token model" })).toBeNull();
    expect(screen.getByText("Cache share 20%")).toBeInTheDocument();
    expect(
      screen.getByText("7 traces, 1 failed, over the window."),
    ).toBeInTheDocument();
  });

  it("says a latency chart holds nothing where no bar read one", () => {
    render(
      withStore(
        <DashboardCharts
          bars={[]}
          tokenBars={[]}
          firstToken={[]}
          hourly={false}
          firstTokenHourly={false}
          histogram={null}
        />,
      ),
    );
    expect(screen.getAllByText("None read in the window.")).toHaveLength(2);
    expect(
      screen.getByText(/Output speed could not be read/),
    ).toBeInTheDocument();
  });
});
