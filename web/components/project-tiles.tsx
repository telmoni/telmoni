import type { ReactNode } from "react";
import Link from "next/link";
import { Settings } from "lucide-react";

import { ContentCue } from "@/components/content-cue";
import { RoleBadge } from "@/components/role-badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { projectPath } from "@/lib/slug";
import type { ContentMode, Role } from "@/lib/types/enums";

export interface ProjectTile {
  id: string;
  slug: string;
  name: string;
  role: Role;
  content_mode: ContentMode;
}

// A tile a project, three to a row from `lg`: the whole tile is the link,
// with its settings' gear and any control a page adds (Leave) beside it
// inside the card, since neither can live inside a link. One column below
// `sm` is declared, or a long name would push the page sideways on a phone.
export function ProjectTiles({
  organization,
  projects,
  action,
  roles = true,
}: {
  organization: string;
  projects: readonly ProjectTile[];
  action?: (project: ProjectTile) => ReactNode;
  /** Whether each tile names the person's role on it; an overview's name the project alone. */
  roles?: boolean;
}) {
  return (
    <ul className="grid grid-cols-1 gap-page sm:grid-cols-2 lg:grid-cols-3">
      {projects.map((project) => {
        const control = action?.(project);
        return (
          <li key={project.id}>
            <Card className="flex flex-row items-center gap-0 overflow-hidden p-0 transition-colors hover:bg-accent focus-within:bg-accent">
              <Link
                href={projectPath(organization, project.slug)}
                className="flex min-w-0 flex-1 items-center justify-between gap-gutter py-gutter pr-1 pl-gutter text-sm"
              >
                <span className="min-w-0 truncate font-medium">{project.name}</span>
                <span className="flex shrink-0 items-center gap-gutter">
                  {roles && <RoleBadge role={project.role} />}
                  <ContentCue mode={project.content_mode} />
                </span>
              </Link>
              {/* The gear's 32px box holds its icon 8px in: 4px more puts the
                  icon a gutter from the eye and from the edge, and a control
                  after it sits a gutter from the edge too. */}
              <div className="flex shrink-0 items-center gap-gutter pr-1">
                <Button
                  asChild
                  variant="ghost"
                  size="icon"
                  className="text-muted-foreground hover:bg-foreground/10 hover:text-foreground"
                >
                  <Link
                    href={projectPath(organization, project.slug, "/settings")}
                    aria-label={`${project.name} settings`}
                    title={`${project.name} settings`}
                  >
                    <Settings />
                  </Link>
                </Button>
                {control && <span className="mr-2">{control}</span>}
              </div>
            </Card>
          </li>
        );
      })}
    </ul>
  );
}
