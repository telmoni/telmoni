import { describe, expect, it } from "vitest";

import { footerColumns } from "./footer";
import { DOCS_URL, REPO_URL } from "./site";

describe("footerColumns", () => {
  it("draws only what an unconfigured deployment has: the product's own links, no legal or contact column", () => {
    const columns = footerColumns({});
    expect(columns.map((column) => column.title)).toEqual(["Resources"]);
    expect(columns[0].links).toEqual(
      expect.arrayContaining([
        expect.objectContaining({ label: "Docs", href: DOCS_URL }),
        expect.objectContaining({ label: "GitHub", href: REPO_URL }),
      ]),
    );
  });

  it("draws the legal and contact columns from the deployment's legal base and address", () => {
    const columns = footerColumns({
      legalUrl: "https://example.com/legal",
      supportEmail: "support@example.com",
    });
    const links = (title: string) =>
      columns.find((column) => column.title === title)?.links ?? [];
    expect(links("Legal")).toEqual([
      expect.objectContaining({
        label: "Privacy Policy",
        href: "https://example.com/legal/privacy-policy",
      }),
      expect.objectContaining({
        label: "Terms of Service",
        href: "https://example.com/legal/terms-of-service",
      }),
    ]);
    expect(links("Contact")).toEqual([
      expect.objectContaining({ label: "Email", href: "mailto:support@example.com" }),
    ]);
  });
});
