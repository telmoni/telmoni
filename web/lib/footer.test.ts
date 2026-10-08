import { describe, expect, it } from "vitest";

import { footerColumns, footerLegal, type FooterColumn } from "./footer";
import { DOCS_URL, REPO_URL } from "./site";

const labels = (columns: readonly FooterColumn[], title: string) =>
  columns.find((column) => column.title === title)?.links.map((link) => link.label) ?? [];

const configured = { legalUrl: "https://example.com/legal", supportEmail: "support@example.com" };

describe("footerColumns", () => {
  it("draws only what an unconfigured deployment has: the product's own links, no company column and no legal line", () => {
    const columns = footerColumns({}, []);
    expect(columns.map((column) => column.title)).toEqual(["Product", "Developers", "Resources"]);
    expect(columns[0]?.links).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ label: "Overview", href: "/" }),
        expect.objectContaining({ label: "Open source", href: REPO_URL }),
      ]),
    );
    expect(columns[1]?.links[0]).toEqual(
      expect.objectContaining({ label: "Documentation", href: DOCS_URL }),
    );
    expect(footerLegal({}, [])).toEqual([]);
  });

  it("draws the company column and the legal line from the deployment's address and legal base", () => {
    expect(labels(footerColumns(configured, []), "Company")).toEqual(["Contact"]);
    expect(footerColumns(configured, []).find((column) => column.title === "Company")?.links).toEqual([
      expect.objectContaining({ label: "Contact", href: "mailto:support@example.com" }),
    ]);
    expect(footerLegal(configured, [])).toEqual([
      expect.objectContaining({ label: "Terms", href: "https://example.com/legal/terms-of-service" }),
      expect.objectContaining({ label: "Privacy", href: "https://example.com/legal/privacy-policy" }),
    ]);
  });

  // A console built on this one names its own pages: each joins the column it
  // names before the link it names, a column the core does not draw goes
  // after the core's, and `Legal` joins the line at the foot.
  it("joins a console's extra links where each says", () => {
    const extras = [
      { column: "Product", label: "Plans", href: "/plans", before: "Self-hosting" },
      { column: "Company", label: "About", href: "/about" },
      {
        column: "Legal",
        label: "Subprocessors",
        href: "https://example.com/legal/subprocessors",
        newTab: true,
      },
      { column: "Partners", label: "Resellers", href: "/resellers" },
    ];
    const columns = footerColumns(configured, extras);
    expect(labels(columns, "Product").slice(0, 2)).toEqual(["Overview", "Plans"]);
    expect(labels(columns, "Company")).toEqual(["Contact", "About"]);
    expect(columns.map((column) => column.title)).toEqual([
      "Product",
      "Developers",
      "Resources",
      "Company",
      "Partners",
    ]);
    expect(footerLegal(configured, extras).map((link) => link.label)).toEqual([
      "Terms",
      "Privacy",
      "Subprocessors",
    ]);
    // The core's own slot is empty, so the default draws the core alone.
    expect(labels(footerColumns(configured), "Product")).not.toContain("Plans");
  });
});
