// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

const push = vi.hoisted(() => vi.fn());
vi.mock("next/navigation", () => ({
  useRouter: () => ({ push }),
  usePathname: () => "/acme/web/dashboards",
  useSearchParams: () => new URLSearchParams("days=30&view=d-1&chart=spend&chart=tokens"),
}));

import { ChartsPicker } from "./_charts-picker";

function open(charts: Parameters<typeof ChartsPicker>[0]["charts"]) {
  render(<ChartsPicker charts={charts} tagKeys={["team"]} />);
  fireEvent.keyDown(screen.getByRole("button", { name: "Charts" }), { key: "Enter" });
}

beforeEach(() => vi.clearAllMocks());

describe("ChartsPicker", () => {
  it("adds a split after the charts drawn, keeping the window and the dashboard named", async () => {
    open([{ measure: "spend" }, { measure: "tokens" }]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "User" }));
    });
    expect(push).toHaveBeenCalledWith(
      "/acme/web/dashboards?days=30&view=d-1&chart=spend&chart=tokens&chart=spend%3Auser",
      { scroll: false },
    );
  });

  it("takes a chart off, and keeps the last one on", async () => {
    open([{ measure: "spend" }, { measure: "tokens" }]);
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitemcheckbox", { name: "Spend" }));
    });
    expect(push).toHaveBeenCalledWith("/acme/web/dashboards?days=30&view=d-1&chart=tokens", { scroll: false });

    push.mockClear();
    cleanup();
    open([{ measure: "runs" }]);
    const last = screen.getByRole("menuitemcheckbox", { name: "Runs" });
    expect(last).toHaveAttribute("aria-checked", "true");
    expect(last).toHaveAttribute("data-disabled");
  });
});
