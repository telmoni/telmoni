import type { ReactNode } from "react";
import Link from "next/link";

import { RoleBadge } from "@/components/role-badge";
import { Card } from "@/components/ui/card";
import { projectPath } from "@/lib/slug";

export interface ProjectTile {
  id: string;
  slug: string;
  name: string;
  role: string | null | undefined;
}

// A tile a project, three to a row from `lg`, so a dozen fit a screen: the
// whole tile is the link, as a tile is expected to be, with the card's own
// ring and corners and the fill under the pointer clipped to them. A control
// a page adds — Leave, on the organization's Projects page — stands beside
// the link inside the card, since a button cannot live inside a link. One
// column below `sm` is still a declared one: the implicit column would grow
// to a long name's full width and push the page sideways on a phone.
export function ProjectTiles({
  organization,
  projects,
  action,
}: {
  organization: string;
  projects: readonly ProjectTile[];
  action?: (project: ProjectTile) => ReactNode;
}) {
  return (
    <ul className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {projects.map((project) => {
        const control = action?.(project);
        return (
          <li key={project.id}>
            <Card className="flex flex-row items-center overflow-hidden p-0 gap-0">
              <Link
                href={projectPath(organization, project.slug)}
                className="flex min-w-0 flex-1 items-center justify-between gap-3 p-4 text-sm transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
              >
                <span className="min-w-0 truncate font-medium">{project.name}</span>
                <RoleBadge role={project.role} />
              </Link>
              {control && <div className="shrink-0 pr-3">{control}</div>}
            </Card>
          </li>
        );
      })}
    </ul>
  );
}
