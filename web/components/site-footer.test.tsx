// @vitest-environment jsdom
import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { SiteFooter } from "./site-footer";

describe("SiteFooter", () => {
  const originalEnv = { ...process.env };

  beforeEach(() => {
    delete process.env.LEGAL_URL;
    delete process.env.SUPPORT_EMAIL;
  });

  afterEach(() => {
    process.env = { ...originalEnv };
  });

  it("draws only what an unconfigured deployment has: the docs, no legal or contact column", () => {
    render(<SiteFooter />);
    expect(screen.getByRole("contentinfo")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Docs" })).toHaveAttribute("href", "https://docs.telmoni.com");
    expect(screen.queryByRole("navigation", { name: "Legal" })).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Contact" })).not.toBeInTheDocument();
  });

  it("draws the legal and contact columns from LEGAL_URL and SUPPORT_EMAIL", () => {
    process.env.LEGAL_URL = "https://example.com/legal";
    process.env.SUPPORT_EMAIL = "support@example.com";

    render(<SiteFooter />);
    expect(screen.getByRole("link", { name: "Privacy Policy" })).toHaveAttribute(
      "href",
      "https://example.com/legal/privacy-policy",
    );
    expect(screen.getByRole("link", { name: "Terms of Service" })).toHaveAttribute(
      "href",
      "https://example.com/legal/terms-of-service",
    );
    expect(screen.getByRole("link", { name: "Email" })).toHaveAttribute(
      "href",
      "mailto:support@example.com",
    );
  });
});
