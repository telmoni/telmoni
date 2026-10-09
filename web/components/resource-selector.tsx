"use client";

import { useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { Check, ChevronDown, Plus, Settings } from "lucide-react";

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
import { CreateProjectDialog, useCanCreateProjects } from "@/components/create-project";
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
  "group h-9 min-w-0 shrink justify-start gap-1.5 overflow-hidden rounded-md font-normal",
  "transition-none hover:bg-transparent data-[state=open]:bg-transparent",
);
const CHEVRON =
  "size-4 shrink-0 opacity-50 transition-opacity group-hover:opacity-100 group-data-[state=open]:opacity-100";

// A 36px trigger centred in the 59px above the header's 1px rule ends 12.5px
// short of the header's bottom edge, so a menu opens right under the rule,
// like the bell's and the account's.
const MENU_OFFSET = 12.5;

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
// from the keyboard: the name opens the resource, the gear its settings.
// Under the pointer, or with either target focused, the whole row fills —
// one fill across the name and the gear, since the row is one thing — and
// the gear's own small box, set 4px in from the row's top, bottom and right
// as Langfuse sets its own, darkens a step more under the pointer. The one
// you stand in carries a check after its name rather than a fill, so hover
// and current never look alike. Neither item fills on its own: the menu's
// per-item fill is turned off here and the row carries it.
function ResourceRow({ row, settingsLabel }: { row: Row; settingsLabel: string }) {
  return (
    <div className="flex items-center gap-0.5 rounded-md hover:bg-accent/50 focus-within:bg-accent/50">
      <DropdownMenuItem
        asChild
        className="h-9 min-w-0 flex-1 cursor-pointer gap-2 px-2 focus:bg-transparent [&_svg]:size-3.5"
      >
        <Link href={row.href} aria-current={row.active ? "true" : undefined}>
          <span className="min-w-0 flex-1 truncate">{row.label}</span>
          {row.active && <Check className="shrink-0" />}
        </Link>
      </DropdownMenuItem>
      {row.settingsHref && (
        <DropdownMenuItem
          asChild
          className={cn(
            // `mt-0!`: an item that follows another gets `mt-0.5` from the
            // menu's own rule, meant for stacked rows; this one stands beside
            // its sibling, and the margin would drop the gear 2px. The box
            // is 28px, 4px in from a 36px row, its icon 14px, a step under the menu's 16px rule,
            // since it is a secondary target beside the name.
            "mt-0! mr-1 size-7 shrink-0 cursor-pointer justify-center p-0 [&_svg]:size-3.5",
            "text-muted-foreground focus:bg-transparent hover:bg-foreground/10 hover:text-foreground",
            "focus-visible:bg-foreground/10 focus-visible:text-foreground",
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
        className="h-9 cursor-pointer gap-2 px-2"
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
// built on this one draws beside its name, and the project after a slash —
// inside one, its name; at the organization's own pages, a prompt to pick
// one, so a project is a click away from anywhere in it rather than a trip
// to Projects. Each menu lists its own kind alone — the organizations the
// person can stand in, the projects of the one they are in — with a gear to
// each one's settings, and the one thing it creates at its foot.
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
  // The dialog creates in the organization the switcher stands in, whose
  // projects its menu lists, so the row is offered to whoever may create there.
  const canCreate = useCanCreateProjects();
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
  // The slash and its menu: a project's name inside one, the prompt at the
  // organization level. A project page naming one the listing lacks draws
  // neither, rather than a prompt that would say you stand in none.
  const projectSlot = activeProject !== undefined || here?.kind === "organization";

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


  return (
    <div className="flex min-w-0 items-center gap-1">
      <DropdownMenu modal={false} open={organizationsOpen} onOpenChange={setOrganizationsOpen}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="default"
            // `px-2`, with the header cell's `pl-1.5`: the name starts where
            // the page's title does.
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

      {projectSlot && (
        <>
          <span aria-hidden="true" className="shrink-0 text-muted-foreground/50">
            /
          </span>
          <DropdownMenu modal={false} open={projectsOpen} onOpenChange={setProjectsOpen}>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="default"
                className={cn(
                  TRIGGER,
                  "px-2.5 has-[>svg]:px-2.5",
                  // The chevron alone on a phone: a whole square to tap,
                  // never squeezed by the organization's name beside it.
                  !activeProject && "max-md:shrink-0",
                )}
                aria-label="Select a project"
              >
                {activeProject ? (
                  <span className="truncate font-medium">{activeProject.name}</span>
                ) : (
                  // A prompt, not a name: muted and at the body's weight, as a
                  // field's placeholder is, so a project that happens to be
                  // called this never reads as the one you stand in. Below
                  // `md` the words go and the chevron stays: on a phone's bar
                  // they and the organization's name cut each other to a
                  // letter apiece.
                  <span
                    className="hidden truncate text-muted-foreground md:block"
                    data-testid="project-prompt"
                  >
                    Select a project
                  </span>
                )}
                <ChevronDown className={CHEVRON} />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent className="w-72 animate-none!" align="start" side="bottom" sideOffset={MENU_OFFSET}>
              <DropdownMenuGroup>
                <DropdownMenuLabel className="px-2 py-1.5 text-xs font-normal text-muted-foreground">
                  Projects
                </DropdownMenuLabel>
                {projectRows.length === 0 && (
                  <p className="flex h-9 items-center px-2 text-sm text-muted-foreground">
                    {canCreate ? "No projects yet" : "No projects you can open"}
                  </p>
                )}
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
