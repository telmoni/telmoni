// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { AuditPagination, auditHref } from "@/components/audit-pagination";

vi.mock("next/navigation", () => ({
  useRouter: () => ({ push: vi.fn() }),
}));

const BASE = "/acme/audit-log";

function draw(page: number, limit = 50, total = 120) {
  render(<AuditPagination base={BASE} total={total} page={page} limit={limit} />);
  const link = (name: string) => screen.queryByRole("link", { name })?.getAttribute("href");
  const off = (name: string) => screen.queryByRole("button", { name })?.hasAttribute("disabled");
  return { link, off };
}

describe("a list's pages", () => {
  it("names the total, the page and how many there are", () => {
    draw(2);
    expect(document.body.textContent).toContain("Total 120");
    expect(document.body.textContent).toContain("Page 2 of 3");
  });

  it("has nowhere back to go on the first page", () => {
    const { link, off } = draw(1);
    expect(off("First page")).toBe(true);
    expect(off("Previous page")).toBe(true);
    expect(link("Next page")).toBe(`${BASE}?page=2`);
    expect(link("Last page")).toBe(`${BASE}?page=3`);
  });

  it("links every way from a middle page, the first page spelled bare", () => {
    const { link } = draw(2);
    expect(link("First page")).toBe(BASE);
    expect(link("Previous page")).toBe(BASE);
    expect(link("Next page")).toBe(`${BASE}?page=3`);
    expect(link("Last page")).toBe(`${BASE}?page=3`);
  });

  it("has nowhere on to go on the last page, nor on one past it", () => {
    for (const page of [3, 9]) {
      const { link, off } = draw(page);
      expect(off("Next page")).toBe(true);
      expect(off("Last page")).toBe(true);
      expect(link("First page")).toBe(BASE);
      document.body.innerHTML = "";
    }
  });

  it("carries a row count that is not the default on every link", () => {
    const { link } = draw(2, 25);
    expect(link("First page")).toBe(`${BASE}?limit=25`);
    expect(link("Next page")).toBe(`${BASE}?page=3&limit=25`);
    expect(document.body.textContent).toContain("Page 2 of 5");
  });
});

describe("an audit page's URL", () => {
  it("names a page and a row count, and neither at its default", () => {
    expect(auditHref(BASE, {})).toBe(BASE);
    expect(auditHref(BASE, { page: 4 })).toBe(`${BASE}?page=4`);
    expect(auditHref(BASE, { limit: 100 })).toBe(`${BASE}?limit=100`);
  });
});
