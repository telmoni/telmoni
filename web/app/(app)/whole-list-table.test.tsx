// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

let search = "";
vi.mock("next/navigation", () => ({
  usePathname: () => "/acme/web/rules",
  useSearchParams: () => new URLSearchParams(search),
}));

import { WholeListTable } from "@/components/whole-list-table";
import { SortDirection } from "@/lib/types/enums";

type Row = { id: string; name: string; share: number };
const ROWS: Row[] = [
  { id: "1", name: "beta", share: 0.5 },
  { id: "2", name: "alpha", share: 0.1 },
  { id: "3", name: "gamma", share: 0.9 },
];

function table() {
  render(
    <WholeListTable
      rows={ROWS}
      rowKey={(row) => row.id}
      defaultSort="name"
      searchLabel="Search rules"
      searchOf={(row) => row.name}
      columns={[
        { key: "name", label: "Name", sort: { by: (row) => row.name, first: SortDirection.Asc }, cell: (row) => row.name },
        { key: "share", label: "Share", numeric: true, sort: { by: (row) => row.share, first: SortDirection.Desc }, cell: (row) => String(row.share) },
      ]}
    />,
  );
  return screen.getAllByRole("row").slice(1).map((row) => row.textContent);
}

beforeEach(() => {
  search = "";
});

describe("WholeListTable", () => {
  it("stands in its default sort, each heading a link to the other way or its own first way", () => {
    expect(table()).toEqual(["alpha0.1", "beta0.5", "gamma0.9"]);
    expect(screen.getByRole("link", { name: "Name" })).toHaveAttribute("href", "/acme/web/rules?sort=name&dir=desc");
    expect(screen.getByRole("link", { name: "Share" })).toHaveAttribute("href", "/acme/web/rules?sort=share&dir=desc");
  });

  it("sorts as the address says, and ignores a column it does not have", () => {
    search = "sort=share&dir=desc";
    expect(table()).toEqual(["gamma0.9", "beta0.5", "alpha0.1"]);
    search = "sort=nonsense&dir=sideways";
    document.body.innerHTML = "";
    expect(table()).toEqual(["alpha0.1", "beta0.5", "gamma0.9"]);
  });

  it("narrows to what is searched for, and says when nothing matches", () => {
    table();
    fireEvent.change(screen.getByLabelText("Search rules"), { target: { value: "GAMMA" } });
    expect(screen.getAllByRole("row").slice(1).map((row) => row.textContent)).toEqual(["gamma0.9"]);
    fireEvent.change(screen.getByLabelText("Search rules"), { target: { value: "zzz" } });
    expect(screen.getByText("Nothing matches.")).toBeInTheDocument();
  });
});
