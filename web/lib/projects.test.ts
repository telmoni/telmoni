import { describe, expect, it } from "vitest";

import { compareProjectsByOrganization } from "./projects";

const project = (name: string, organizationName: string) => ({
  name,
  organizationName,
});

const order = (projects: ReturnType<typeof project>[]) =>
  [...projects].sort(compareProjectsByOrganization).map((p) => p.name);

describe("compareProjectsByOrganization", () => {
  // Grouped under what the switcher prints for each organization, so a group
  // sorts where its heading reads: the name its owner chose.
  it("groups by the organization's label, its name", () => {
    expect(
      order([project("Ops", "Zeta"), project("Data", "Acme"), project("Web", "Beta")]),
    ).toEqual(["Data", "Web", "Ops"]);
  });

  it("orders projects by name within one organization, ignoring case", () => {
    expect(
      order([project("web", "Acme"), project("Api", "Acme"), project("Data", "Acme")]),
    ).toEqual(["Api", "Data", "web"]);
  });
});
