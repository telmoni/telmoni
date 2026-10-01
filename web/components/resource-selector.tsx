"use client";

import { Fragment, useEffect, useRef, useState } from "react";
import Link from "next/link";
import { useSelectedLayoutSegment } from "next/navigation";
import { Building2, Check, ChevronsUpDown, Plus, SquareStack } from "lucide-react";

import { switchActiveOrganizationAction } from "@/app/(app)/actions";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Kbd } from "@/components/ui/kbd";
import { CreateProjectDialog, useCreateProjectTargets } from "@/components/create-project";
import { RoleBadge } from "@/components/role-badge";
import { isOrganizationSegment } from "@/lib/console-nav";
import { resourceUrl, standingResource } from "@/lib/console-trail";
import { organizationLabel } from "@/lib/identity";
import { resolveActiveOrganization } from "@/lib/organization-label";
import type { OrganizationRole } from "@/lib/organization-role";
import type { ProjectEverywhere } from "@/lib/server/entities/projects";
import {
  useActiveOrganizationId,
  useOrganizations,
  useProjects,
  useProjectsElsewhere,
} from "@/lib/store";
import { useConsoleTrail } from "@/lib/use-console-trail";
import { cn } from "@/lib/utils";

function firstRow(e: React.KeyboardEvent<HTMLElement>): HTMLElement | null {
  return (
    e.currentTarget
      .closest<HTMLElement>('[role="menu"]')
      ?.querySelector<HTMLElement>("[data-resource-row]:not([data-context])") ??
    null
  );
}

type OtherOrganization = {
  id: string;
  label: string;
  // `null` when reached through a project alone, with no organization
  // membership to show.
  role: OrganizationRole | null;
  projects: ProjectEverywhere[];
};

export function ResourceSelector() {
  const [open, setOpen] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [query, setQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const projects = useProjects();
  const elsewhere = useProjectsElsewhere();
  // Any organization the caller may create in, not only the active one: the
  // dialog asks which.
  const canCreate = useCreateProjectTargets().length > 0;
  const organizations = useOrganizations();
  const activeOrganizationId = useActiveOrganizationId();
  const segment = useSelectedLayoutSegment() ?? "";
  // A row returns to the page you last had open in that resource.
  const trail = useConsoleTrail();
  // Account pages name no resource, so the trigger keeps naming the one the
  // sidebar's back row points at.
  const resourceSegment = standingResource(
    segment,
    trail,
    projects.map((p) => p.id),
  );
  const activeProject = projects.find((p) => p.id === resourceSegment);
  const atOrganization = isOrganizationSegment(resourceSegment);

  const { active, label: activeOrganizationName } = resolveActiveOrganization({
    activeOrganizationId,
    organizations,
  });
  // Every organization row shows your role in it, which is how yours are told
  // apart from ones you were invited into.
  const activeOrganizationRole: OrganizationRole | null = active?.role ?? null;

  const seen = new Set<string>();
  const otherOrganizations: OtherOrganization[] = [];
  const add = (id: string, label: string, role: OrganizationRole | null) => {
    if (!id || id === activeOrganizationId || seen.has(id)) return;
    seen.add(id);
    otherOrganizations.push({
      id,
      label,
      role,
      projects: elsewhere.filter((t) => t.organizationId === id),
    });
  };
  for (const o of organizations) {
    add(o.organizationId, organizationLabel(o), o.role);
  }
  for (const t of elsewhere) {
    add(
      t.organizationId,
      organizationLabel({ name: t.organizationName, ownerEmail: t.organizationOwnerEmail }),
      null,
    );
  }

  const needle = query.trim().toLowerCase();
  const has = (s: string) => s.toLowerCase().includes(needle);
  const organizationSelfMatches = !needle || has(activeOrganizationName);
  const matches = organizationSelfMatches ? projects : projects.filter((t) => has(t.name));
  const showActiveOrganization = organizationSelfMatches || matches.length > 0;
  const activeOrganizationIsContext = !organizationSelfMatches;
  const otherMatches = otherOrganizations
    .map((org) => {
      const self = !needle || has(org.label);
      return {
        ...org,
        projects: self ? org.projects : org.projects.filter((t) => has(t.name)),
        context: !self,
      };
    })
    .filter((org) => !org.context || org.projects.length > 0);
  const nothingMatches = !showActiveOrganization && otherMatches.length === 0;

  useEffect(() => {
    if (open) inputRef.current?.focus({ preventScroll: true });
  }, [open]);

  function onFilterKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      firstRow(e)?.focus();
      return;
    }
    if (e.key === "Enter") {
      const row = firstRow(e);
      if (row) {
        e.preventDefault();
        row.click();
      }
      return;
    }
    if (e.key.length === 1) e.stopPropagation();
  }

  return (
    <DropdownMenu
      modal={false}
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setQuery("");
      }}
    >
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="default"
          className={cn(
            // `shrink` (against Button's `shrink-0`): the one header control
            // that gives way, so a long name truncates instead of pushing into
            // the icon buttons. `pl-4` lines the name up with the page title
            // below (`<main>`'s `p-4`). `gap-0` overrides Button's `gap-2`.
            "h-8 min-w-0 shrink overflow-hidden font-normal justify-start gap-0 rounded-menu",
            "pl-4 pr-3.5 has-[>svg]:pl-4 has-[>svg]:pr-3.5",
            "hover:bg-sidebar-accent",
            "transition-none",
            "data-[state=open]:bg-sidebar-accent",
          )}
          aria-label="Select a resource"
        >
          <div className="flex flex-1 items-center gap-1.5 min-w-0 pr-1 text-left">
            <span className="truncate font-semibold">
              {atOrganization
                ? activeOrganizationName
                : (activeProject?.name ?? "Project")}
            </span>
            {!atOrganization && (
              <RoleBadge
                className="h-4 px-1"
                role={activeProject?.role}
                testId="resource-selector-active-badge"
              />
            )}
          </div>
          <ChevronsUpDown className="ml-2 size-4 shrink-0 opacity-50" />
        </Button>
      </DropdownMenuTrigger>

      {/* 14 = the header's `py-3.5`, so the menu opens on the header's
          bottom edge, like the bell's and account's. */}
      <DropdownMenuContent
        className="w-80 p-0 animate-none!"
        align="start"
        side="bottom"
        sideOffset={14}
      >
        <div className="flex h-14 items-center gap-2 border-b px-3">
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onFilterKeyDown}
            placeholder="Find a resource…"
            aria-label="Find a resource"
            autoComplete="off"
            spellCheck={false}
            className="h-8 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
          />
          <Kbd>Esc</Kbd>
        </div>

        <div className="max-h-80 overflow-y-auto">
          {showActiveOrganization && (
            <DropdownMenuGroup className="p-1">
              <DropdownMenuItem asChild className="h-8 gap-2 px-2 cursor-pointer">
                <Link
                  href={resourceUrl(trail, "organization")}
                  data-resource-row="organization"
                  data-context={activeOrganizationIsContext || undefined}
                >
                  <div className="flex size-6 items-center justify-center rounded-sm border shrink-0">
                    <Building2 className="size-3" />
                  </div>
                  <span className="truncate font-medium flex-1 min-w-0">
                    {activeOrganizationName}
                  </span>
                  <span className="shrink-0 text-xs text-muted-foreground">
                    Organization
                  </span>
                  <RoleBadge
                    className="h-4 px-1"
                    role={activeOrganizationRole}
                    testId="resource-selector-organization-badge"
                  />
                  {atOrganization && <Check className="size-4 shrink-0" />}
                </Link>
              </DropdownMenuItem>
              {matches.map((project) => (
                <DropdownMenuItem key={project.id} asChild className="h-8 gap-2 px-2 pl-8 cursor-pointer">
                  <Link href={resourceUrl(trail, project.id)} data-resource-row="project">
                    <div className="flex size-6 items-center justify-center rounded-sm border shrink-0">
                      <SquareStack className="size-3" />
                    </div>
                    <span className="truncate font-medium flex-1 min-w-0">{project.name}</span>
                    <RoleBadge
                      className="h-4 px-1"
                      role={project.role}
                      testId="resource-selector-item-badge"
                    />
                    {project.id === resourceSegment && <Check className="size-4 shrink-0" />}
                  </Link>
                </DropdownMenuItem>
              ))}
            </DropdownMenuGroup>
          )}

          {otherMatches.length > 0 && (
            <DropdownMenuGroup
              className={cn("p-1", showActiveOrganization && "border-t")}
              data-testid="resources-elsewhere"
            >
              <DropdownMenuLabel className="px-2 py-1.5 text-xs font-normal text-muted-foreground">
                Other organizations
              </DropdownMenuLabel>
              {otherMatches.map((org) => (
                <Fragment key={org.id}>
                  <DropdownMenuItem
                    data-resource-row="organization"
                    data-context={org.context || undefined}
                    className="h-8 gap-2 px-2 cursor-pointer"
                    onSelect={() => switchActiveOrganizationAction(org.id)}
                  >
                    <div className="flex size-6 items-center justify-center rounded-sm border shrink-0">
                      <Building2 className="size-3" />
                    </div>
                    <span className="truncate font-medium flex-1 min-w-0">
                      {org.label}
                    </span>
                    <span className="shrink-0 text-xs text-muted-foreground">
                      Organization
                    </span>
                    <RoleBadge
                      className="h-4 px-1"
                      role={org.role}
                      testId="resource-selector-organization-badge"
                    />
                  </DropdownMenuItem>
                  {org.projects.map((project) => (
                    <DropdownMenuItem
                      key={project.id}
                      data-resource-row="project"
                      className="h-8 gap-2 px-2 pl-8 cursor-pointer"
                      onSelect={() =>
                        switchActiveOrganizationAction(org.id, project.id)
                      }
                    >
                      <div className="flex size-6 items-center justify-center rounded-sm border shrink-0">
                        <SquareStack className="size-3" />
                      </div>
                      <span className="truncate font-medium flex-1 min-w-0">
                        {project.name}
                      </span>
                      <RoleBadge
                        className="h-4 px-1"
                        role={project.role}
                        testId="resource-selector-item-badge"
                      />
                    </DropdownMenuItem>
                  ))}
                </Fragment>
              ))}
            </DropdownMenuGroup>
          )}
        </div>

        {nothingMatches ? (
          <p className="px-4 py-6 text-center text-sm text-muted-foreground" role="status">
            No resource matches &ldquo;{query.trim()}&rdquo;.
          </p>
        ) : (
          !needle &&
          projects.length + elsewhere.length <= 1 &&
          otherOrganizations.length === 0 && (
            <div className="flex flex-col items-center gap-3 px-6 py-8 text-center">
              <div className="flex size-10 items-center justify-center rounded-md border">
                <SquareStack className="size-4 text-muted-foreground" />
              </div>
              <p className="text-sm text-muted-foreground">
                Projects you create and join appear here, under the organization
                that owns them.
              </p>
            </div>
          )
        )}

        {canCreate && (
          <DropdownMenuItem
            className="h-14 cursor-pointer gap-3 rounded-none border-t px-3"
            onSelect={(e) => {
              e.preventDefault();
              setOpen(false);
              setCreateOpen(true);
            }}
          >
            <Plus className="size-4 shrink-0" />
            <span className="text-sm font-medium">Create project</span>
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>

      {canCreate && (
        <CreateProjectDialog
          key={`project-${createOpen}`}
          open={createOpen}
          onOpenChange={setCreateOpen}
        />
      )}
    </DropdownMenu>
  );
}
