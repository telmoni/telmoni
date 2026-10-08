"use client";

import { useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { ChevronDown, Plus, Settings } from "lucide-react";

import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { CreateOrganizationDialog } from "@/components/create-organization";
import { CreateProjectDialog, useCreateProjectTargets } from "@/components/create-project";
import { resourceUrl, standingResource } from "@/lib/console-trail";
import { organizationLabel } from "@/lib/identity";
import { resolveActiveOrganization } from "@/lib/organization-label";
import { Flag, flagOn } from "@/lib/flags";
import {
  useActiveOrganizationId,
  useFlags,
  useOrganizations,
  useProjects,
  useProjectsElsewhere,
} from "@/lib/store";
import { organizationPath, projectPath } from "@/lib/slug";
import { useConsoleTrail, useLiveResources } from "@/lib/use-console-trail";
import { cn } from "@/lib/utils";

// The header's two triggers: the one control in the header that gives way
// (`shrink`, against Button's `shrink-0`), so a long name truncates instead
// of pushing into the icon buttons. `gap-1.5` overrides Button's `gap-2`.
// No fill on hover or while open: the name stands where the page title
// does, and a fill drawn around it reads as the name shifting; the chevron
// answers the pointer instead.
const TRIGGER = cn(
  "group h-8 min-w-0 shrink justify-start gap-1.5 overflow-hidden rounded-md font-normal",
  "transition-none hover:bg-transparent data-[state=open]:bg-transparent",
);
const CHEVRON =
  "size-4 shrink-0 opacity-50 transition-opacity group-hover:opacity-100 group-data-[state=open]:opacity-100";

// 14 = the header's `py-3.5`, so a menu opens on the header's bottom edge,
// like the bell's and the account's.
const MENU_OFFSET = 14;

type Row = {
  key: string;
  label: string;
  href: string;
  // Absent where the person has no standing to open the settings: an
  // organization reached through a project alone.
  settingsHref: string | null;
  active: boolean;
};

// A row with two targets, each a menu item of its own so both are reached
// from the keyboard: the name opens the resource, the gear its settings. The
// one you stand in is lit with the menu's own fill, in place of a mark — the
// whole row, the gear being the row's too, and its name in medium — and a
// half under the pointer or focused darkens over that fill, so the row never
// reads as two. The rail's tint is not used: a menu is lighter than the page,
// and the tint all but vanishes on it in the dark.
function ResourceRow({ row, settingsLabel }: { row: Row; settingsLabel: string }) {
  return (
    <div
      className={cn(
        "flex items-center gap-0.5 rounded-md",
        row.active &&
          "bg-accent text-accent-foreground [&_[data-slot=dropdown-menu-item]:focus]:bg-foreground/5",
      )}
    >
      <DropdownMenuItem asChild className="h-8 min-w-0 flex-1 cursor-pointer gap-2 px-2">
        <Link href={row.href} aria-current={row.active ? "true" : undefined}>
          <span className={cn("min-w-0 flex-1 truncate", row.active && "font-medium")}>
            {row.label}
          </span>
        </Link>
      </DropdownMenuItem>
      {row.settingsHref && (
        <DropdownMenuItem
          asChild
          className={cn(
            "size-8 shrink-0 cursor-pointer justify-center p-0",
            !row.active && "text-muted-foreground",
          )}
        >
          <Link href={row.settingsHref} aria-label={settingsLabel} title={settingsLabel}>
            <Settings className="size-4" />
          </Link>
        </DropdownMenuItem>
      )}
    </div>
  );
}

// The foot of a menu: the one thing it creates.
function NewRow({ onSelect, children }: { onSelect: () => void; children: string }) {
  return (
    <>
      <DropdownMenuSeparator />
      <DropdownMenuItem
        className="h-8 cursor-pointer gap-2 px-2"
        onSelect={(e) => {
          e.preventDefault();
          onSelect();
        }}
      >
        <Plus className="size-4 shrink-0 text-muted-foreground" />
        {children}
      </DropdownMenuItem>
    </>
  );
}

// Where you stand, as two menus: the organization, with whatever a console
// built on this one draws beside its name, and, inside a project, the
// project after a slash. Each menu lists its own kind alone — the
// organizations the person can stand in, the projects of the one they are
// in — with a gear to each one's settings, and the one thing it creates at
// its foot.
export function ResourceSelector({
  host,
  organizationBadge,
}: {
  /// The console's host, shown ahead of a new organization's URL.
  host: string;
  /// Drawn beside the organization's name: a console built on this one puts
  /// the organization's plan there (`components/extension/organization-badge.tsx`).
  organizationBadge?: React.ReactNode;
}) {
  const [organizationsOpen, setOrganizationsOpen] = useState(false);
  const [projectsOpen, setProjectsOpen] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [newOrganizationOpen, setNewOrganizationOpen] = useState(false);

  const organizations = useOrganizations();
  const projects = useProjects();
  const elsewhere = useProjectsElsewhere();
  const activeOrganizationId = useActiveOrganizationId();
  // Any organization the caller may create in, not only the active one: the
  // dialog asks which, and opens on the active one when it is among them.
  const targets = useCreateProjectTargets();
  const signupsOpen = flagOn(useFlags(), Flag.Signup);
  // A row returns to the page you last had open in that resource.
  const trail = useConsoleTrail();
  const { live, fallback } = useLiveResources();
  // Account pages name no resource, so the triggers keep naming the one the
  // sidebar's back row points at.
  const standing = standingResource(usePathname(), trail, live, fallback);

  const { active, label: activeOrganizationName } = resolveActiveOrganization({
    activeOrganizationId,
    organizations,
  });
  // `projects` is the listing of the organization the console stands in, so
  // a project is named from it only when it is in that organization.
  const here = active && standing?.organization === active.slug ? standing : null;
  const activeProject =
    here?.kind === "project" ? projects.find((p) => p.slug === here.project) : undefined;

  // Every organization the person can stand in, the active one first: the
  // ones they are in, then the ones they reach through a project alone.
  const organizationRows: Row[] = [];
  const seen = new Set<string>();
  const addOrganization = (id: string, slug: string, label: string, member: boolean) => {
    if (!id || seen.has(id)) return;
    seen.add(id);
    const isActive = id === activeOrganizationId;
    organizationRows.push({
      key: id,
      label,
      href:
        isActive && active
          ? resourceUrl(trail, { kind: "organization", organization: active.slug })
          : organizationPath(slug),
      settingsHref: member ? organizationPath(slug, "/settings") : null,
      active: isActive,
    });
  };
  if (active) addOrganization(active.organizationId, active.slug, activeOrganizationName, true);
  for (const o of organizations) addOrganization(o.organizationId, o.slug, organizationLabel(o), true);
  for (const t of elsewhere) {
    addOrganization(
      t.organizationId,
      t.organizationSlug,
      organizationLabel({ name: t.organizationName }),
      false,
    );
  }

  const projectRows: Row[] = active
    ? projects.map((p) => ({
        key: p.id,
        label: p.name,
        href: resourceUrl(trail, { kind: "project", organization: active.slug, project: p.slug }),
        settingsHref: projectPath(active.slug, p.slug, "/settings"),
        active: p.id === activeProject?.id,
      }))
    : [];

  const canCreate = targets.length > 0;
  const createIn = active && targets.some((t) => t.id === active.organizationId) ? active.organizationId : undefined;

  return (
    <div className="flex min-w-0 items-center gap-1">
      <DropdownMenu modal={false} open={organizationsOpen} onOpenChange={setOrganizationsOpen}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="default"
            // `px-2`, with the header cell's `pl-4`: the name starts where the
            // page's title does.
            className={cn(TRIGGER, "px-2 has-[>svg]:px-2")}
            aria-label="Select an organization"
          >
            <span className="truncate font-medium">{activeOrganizationName}</span>
            {organizationBadge}
            <ChevronDown className={CHEVRON} />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className="w-72 animate-none!" align="start" side="bottom" sideOffset={MENU_OFFSET}>
          <DropdownMenuGroup>
            <DropdownMenuLabel className="px-2 py-1.5 text-xs font-normal text-muted-foreground">
              Organizations
            </DropdownMenuLabel>
            {organizationRows.map((row) => (
              <ResourceRow key={row.key} row={row} settingsLabel={`${row.label} settings`} />
            ))}
          </DropdownMenuGroup>
          {/* Offered to everyone signed in, whatever their roles: the new
              organization is theirs. With the `signup` flag off the act is
              gone, as a switched-off feature is from the rail, rather than a
              row every use of which is refused; a dialog opened before the
              switch still hears auth's refusal. */}
          {signupsOpen && (
            <NewRow
              onSelect={() => {
                setOrganizationsOpen(false);
                setNewOrganizationOpen(true);
              }}
            >
              New organization
            </NewRow>
          )}
        </DropdownMenuContent>
      </DropdownMenu>

      {activeProject && (
        <>
          <span aria-hidden="true" className="shrink-0 text-muted-foreground/50">
            /
          </span>
          <DropdownMenu modal={false} open={projectsOpen} onOpenChange={setProjectsOpen}>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="default"
                className={cn(TRIGGER, "px-2.5 has-[>svg]:px-2.5")}
                aria-label="Select a project"
              >
                <span className="truncate font-medium">{activeProject.name}</span>
                <ChevronDown className={CHEVRON} />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent className="w-72 animate-none!" align="start" side="bottom" sideOffset={MENU_OFFSET}>
              <DropdownMenuGroup>
                <DropdownMenuLabel className="px-2 py-1.5 text-xs font-normal text-muted-foreground">
                  Projects
                </DropdownMenuLabel>
                {projectRows.map((row) => (
                  <ResourceRow key={row.key} row={row} settingsLabel={`${row.label} settings`} />
                ))}
              </DropdownMenuGroup>
              {canCreate && (
                <NewRow
                  onSelect={() => {
                    setProjectsOpen(false);
                    setCreateOpen(true);
                  }}
                >
                  New project
                </NewRow>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        </>
      )}

      {canCreate && (
        <CreateProjectDialog
          key={`project-${createOpen}`}
          open={createOpen}
          onOpenChange={setCreateOpen}
          organizationId={createIn}
        />
      )}
      <CreateOrganizationDialog
        key={`organization-${newOrganizationOpen}`}
        open={newOrganizationOpen}
        onOpenChange={setNewOrganizationOpen}
        host={host}
      />
    </div>
  );
}
