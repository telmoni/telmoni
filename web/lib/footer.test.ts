import { describe, expect, it } from "vitest";

import { footerColumns, type FooterColumn } from "./footer";
import { DOCS_URL, REPO_URL } from "./site";

const labels = (columns: readonly FooterColumn[], title: string) =>
  columns.find((column) => column.title === title)?.links.map((link) => link.label) ?? [];

const configured = { legalUrl: "https://example.com/legal", supportEmail: "support@example.com" };

describe("footerColumns", () => {
  it("draws only what an unconfigured deployment has: the product's own links, and no company column", () => {
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
  });

  it("draws the company column from the deployment's address, and the legal one from its legal base", () => {
    const columns = footerColumns(configured, []);
    expect(columns.map((column) => column.title)).toEqual(["Product", "Developers", "Resources", "Company", "Legal"]);
    expect(columns.find((column) => column.title === "Company")?.links).toEqual([
      expect.objectContaining({ label: "Contact", href: "mailto:support@example.com" }),
    ]);
    expect(columns.find((column) => column.title === "Resources")?.links).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ label: "Security", href: "https://example.com/legal/security", newTab: true }),
      ]),
    );
    expect(columns.find((column) => column.title === "Legal")?.links).toEqual([
      expect.objectContaining({ label: "Terms", href: "https://example.com/legal/terms-of-service", newTab: true }),
      expect.objectContaining({ label: "Privacy", href: "https://example.com/legal/privacy-policy", newTab: true }),
    ]);
  });

  // A console built on this one names its own pages: each joins the column it
  // names before the link it names, and a column the core does not draw goes
  // after the core's.
  it("joins a console's extra links where each says", () => {
    const extras = [
      { column: "Product", label: "Plans", href: "/plans", before: "Self-hosting" },
      { column: "Company", label: "About", href: "/about", before: "Contact" },
      { column: "Partners", label: "Resellers", href: "/resellers" },
    ];
    const columns = footerColumns(configured, extras);
    expect(labels(columns, "Product").slice(0, 2)).toEqual(["Overview", "Plans"]);
    expect(labels(columns, "Company")).toEqual(["About", "Contact"]);
    expect(columns.map((column) => column.title)).toEqual([
      "Product",
      "Developers",
      "Resources",
      "Company",
      "Legal",
      "Partners",
    ]);
    // The core's own slot is empty, so the default draws the core alone.
    expect(labels(footerColumns(configured), "Product")).not.toContain("Plans");
  });
});
