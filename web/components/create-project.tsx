"use client";

import { unstable_rethrow, useRouter } from "next/navigation";
import { FolderPlus, Plus } from "lucide-react";
import { startTransition, useId, useRef, useState, useTransition } from "react";
import { toast } from "sonner";

import { useConsoleUi } from "@/components/console-ui-context";
import { PageAction } from "@/components/page-action";
import {
  SidePanel,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { createProjectAction } from "@/app/(app)/actions";
import { organizationLabel } from "@/lib/identity";
import { projectPath } from "@/lib/slug";
import { useActiveOrganization, useAddProject, useRoles } from "@/lib/store";
import { MAX_PROJECT_NAME, typedBound } from "@/lib/names";
import { administersOrganization } from "@/lib/organization-role";

/// Whether the caller may create a project in the organization they are STANDING
/// IN: every entry point's question, since the panel creates there and asks
/// about no other.
export function useCanCreateProjects(): boolean {
  return administersOrganization(useRoles().organization);
}

/** New project, in the console's side panel (`create-panels.tsx`). */
export function CreateProjectPanel({
  onClose,
  returnFocus,
}: {
  onClose: () => void;
  returnFocus?: () => HTMLElement | null;
}) {
  const router = useRouter();
  const addProject = useAddProject();
  // The project lands in the organization the console stands in, which every
  // way in — the Projects page, and the switcher's project menu, which lists
  // this organization's projects — already stands in; the panel names it.
  const organization = useActiveOrganization();
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const canSubmit = name.trim().length > 0 && organization !== null && !pending;

  function submit() {
    if (!organization) return;
    setError(null);
    start(async () => {
      try {
        const res = await createProjectAction(name.trim(), organization.organizationId);
        if (!res.ok) {
          setError(res.error);
          return;
        }
        // On to the project; an answer that could not be read leaves this
        // page, which the action's revalidation draws again with it listed.
        // The panel shuts in one transition with either, so it goes as the
        // page it leaves for lands rather than a frame before.
        startTransition(() => {
          onClose();
          if (res.project) {
            addProject(res.project);
            router.push(projectPath(organization.slug, res.project.slug));
          }
        });
        toast.success("Project created.");
      } catch (err) {
        unstable_rethrow(err);
        setError("Something went wrong. Please try again.");
      }
    });
  }

  return (
    <SidePanel
      panel={panel}
      titleId={titleId}
      onClose={onClose}
      full
      initialFocus={field}
      returnFocus={returnFocus}
    >
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (canSubmit) submit();
        }}
      >
        <SidePanelHeader icon={FolderPlus} titleId={titleId} title="New project" onClose={onClose}>
          <p className="text-sm text-muted-foreground">
            A project is what every API resource hangs off, with its own keys and members.
          </p>
        </SidePanelHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-3.5">
          <SidePanelRow label="Project name" htmlFor="project-name">
            <Input
              ref={field}
              id="project-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              // An example of what a project is here: the agent it watches.
              placeholder="Support agent"
              maxLength={typedBound(MAX_PROJECT_NAME)}
              autoComplete="off"
              required
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? "project-name-error" : "project-name-hint"}
              className="max-w-md"
            />
            {error ? (
              <p id="project-name-error" className="text-destructive" role="alert">
                {error}
              </p>
            ) : (
              <p id="project-name-hint" className="text-muted-foreground">
                You can rename it later in its settings.
              </p>
            )}
          </SidePanelRow>
          <SidePanelRow label="Organization">
            <span className="truncate">{organization ? organizationLabel(organization) : "None"}</span>
          </SidePanelRow>
          <SidePanelRow label="Content mode">
            <span>
              Off, the mode every project starts in: the content its spans carry is dropped as they
              arrive.
            </span>
          </SidePanelRow>
        </div>
        <SidePanelFooter>
          <Button type="submit" disabled={!canSubmit}>
            {pending ? "Creating…" : "Create project"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}

export function CreateProjectAction() {
  const canCreate = useCanCreateProjects();
  const { openCreate } = useConsoleUi();

  if (!canCreate) return null;

  return (
    <PageAction primary="New project" onClick={(e) => openCreate("project", e.currentTarget)}>
      <Plus className="size-4" />
      New project
    </PageAction>
  );
}
