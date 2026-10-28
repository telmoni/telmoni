import { resourceUrl, type ConsoleTrail } from "@/lib/console-trail";
import { organizationLabel } from "@/lib/identity";
import { organizationPath, projectPath } from "@/lib/slug";

/** A row of the header's two menus: where its name leads, its settings
 *  where the person has standing to open them, and whether it is the one
 *  they stand in. */
export type SwitcherRow = {
  key: string;
  label: string;
  href: string;
  /** Absent for an organization reached through a project alone. */
  settingsHref: string | null;
  active: boolean;
};

type Organization = { organizationId: string; slug: string; name: string };

/** Every organization the person can stand in, each once, the one they stand
 *  in first: the ones they are in, then the ones they reach through a project
 *  alone, which open no settings. The active one's row returns to the page
 *  last open in it, as the trail has it; the others open at their overview. */
export function organizationRows({
  active,
  organizations,
  elsewhere,
  trail,
}: {
  active: Organization | null;
  organizations: readonly Organization[];
  elsewhere: readonly {
    organizationId: string;
    organizationSlug: string;
    organizationName: string;
  }[];
  trail: ConsoleTrail;
}): SwitcherRow[] {
  const rows: SwitcherRow[] = [];
  const seen = new Set<string>();
  const add = (organization: Organization, member: boolean) => {
    const { organizationId: id, slug } = organization;
    if (!id || seen.has(id)) return;
    seen.add(id);
    const isActive = id === active?.organizationId;
    rows.push({
      key: id,
      label: organizationLabel(organization),
      href: isActive
        ? resourceUrl(trail, { kind: "organization", organization: slug })
        : organizationPath(slug),
      settingsHref: member ? organizationPath(slug, "/settings") : null,
      active: isActive,
    });
  };
  if (active) add(active, true);
  for (const organization of organizations) add(organization, true);
  for (const project of elsewhere) {
    add(
      {
        organizationId: project.organizationId,
        slug: project.organizationSlug,
        name: project.organizationName,
      },
      false,
    );
  }
  return rows;
}

/** The projects of the organization the person stands in, as its menu lists
 *  them, each returning to the page last open in it; none outside one. */
export function projectRows({
  active,
  projects,
  activeProjectId,
  trail,
}: {
  active: { slug: string } | null;
  projects: readonly { id: string; slug: string; name: string }[];
  activeProjectId: string | null;
  trail: ConsoleTrail;
}): SwitcherRow[] {
  if (!active) return [];
  return projects.map((project) => ({
    key: project.id,
    label: project.name,
    href: resourceUrl(trail, {
      kind: "project",
      organization: active.slug,
      project: project.slug,
    }),
    settingsHref: projectPath(active.slug, project.slug, "/settings"),
    active: project.id === activeProjectId,
  }));
}
