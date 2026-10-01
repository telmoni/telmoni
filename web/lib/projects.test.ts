import { describe, expect, it } from "vitest";

import { compareProjectsByOrganization } from "./projects";

const project = (
  name: string,
  organizationName: string | null,
  organizationOwnerEmail: string | null,
) => ({ name, organizationName, organizationOwnerEmail });

const order = (projects: ReturnType<typeof project>[]) =>
  [...projects].sort(compareProjectsByOrganization).map((p) => p.name);

describe("compareProjectsByOrganization", () => {
  // Grouped under what the switcher prints for each organization, so a group
  // sorts where its heading reads: the name its owner chose, else the owner's
  // address — never the owner's own name, which labels nothing here.
  it("groups by the organization's label: its name, else its owner's address", () => {
    expect(
      order([
        project("Ops", null, "zed@example.test"),
        project("Data", "Acme", "yolanda@example.test"),
        project("Web", null, "bea@example.test"),
      ]),
    ).toEqual(["Data", "Web", "Ops"]);
  });

  it("orders projects by name within one organization, ignoring case", () => {
    expect(
      order([
        project("web", "Acme", "a@example.test"),
        project("Api", "Acme", "a@example.test"),
        project("Data", "Acme", "a@example.test"),
      ]),
    ).toEqual(["Api", "Data", "web"]);
  });

  it("files an organization read mid-transfer, with no owner, under Organization", () => {
    expect(
      order([
        project("Late", null, "zed@example.test"),
        project("Orphan", null, null),
        project("Early", null, "abe@example.test"),
      ]),
    ).toEqual(["Early", "Orphan", "Late"]);
  });
});
