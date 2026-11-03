"use client";

import { useRouter } from "next/navigation";
import { FolderPlus } from "lucide-react";
import { startTransition, useEffect, useId, useRef, useState } from "react";
import { toast } from "sonner";

import { CodeField, CodeSentTo } from "@/components/code-field";
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
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { createProjectAction } from "@/app/(app)/actions";
import {
  requestContentModeCodeAction,
  updateContentModeAction,
} from "@/app/(app)/[organization]/[project]/settings/actions";
import { CONTENT_MODE_LABEL } from "@/lib/content-mode";
import { organizationLabel } from "@/lib/identity";
import { projectPath } from "@/lib/slug";
import { useActiveOrganization, useAddProject, useUser } from "@/lib/store";
import { MAX_PROJECT_NAME, typedBound } from "@/lib/names";
import { administers } from "@/lib/roles";
import type { Project } from "@/lib/server/entities/projects";
import { ContentMode, OrganizationRole } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";
import { useCodeConfirmed } from "@/lib/use-code-confirmed";

/// Whether the caller may create a project in the organization they are STANDING
/// IN: every entry point's question, since the panel creates there and asks
/// about no other.
export function useCanCreateProjects(): boolean {
  return administers(useActiveOrganization()?.role);
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
  const [mode, setMode] = useState<ContentMode>(ContentMode.Off);
  // A project made to keep content, waiting on the owner's code.
  const [made, setMade] = useState<Project | null>(null);
  const madeId = useRef("");
  const modeId = useId();
  const codeId = useId();
  const email = useUser()?.email ?? "";
  const { pending, error, run } = useAction();
  // Only the owner turns content on, confirmed from their inbox.
  const isOwner = organization?.role === OrganizationRole.Owner;
  const flow = useCodeConfirmed(
    () => requestContentModeCodeAction(madeId.current),
    (code) => updateContentModeAction(madeId.current, ContentMode.On, { code }),
  );

  const canSubmit = name.trim().length > 0 && organization !== null && !pending;

  /** On to the project, the panel shutting as the page it leaves for lands. */
  function open(project: Project | undefined) {
    if (!organization) return;
    startTransition(() => {
      onClose();
      if (project) {
        addProject(project);
        router.push(projectPath(organization.slug, project.slug));
      }
    });
  }

  function submit() {
    if (!organization) return;
    if (made) {
      if (flow.step.step !== "code") flow.sendCode();
      else if (flow.canConfirm) flow.confirmCode();
      return;
    }
    run(
      () => createProjectAction(name.trim(), organization.organizationId),
      (res) => {
        // A project to keep content waits here for the owner's code; an
        // answer that could not be read leaves it off, the page drawn again.
        if (mode === ContentMode.On && res.project) {
          madeId.current = res.project.id;
          setMade(res.project);
          flow.sendCode();
          return;
        }
        open(res.project);
        toast.success("Project created.");
      },
    );
  }

  const confirmed = flow.step.step === "done";
  useEffect(() => {
    if (!confirmed || !made) return;
    open(made);
    toast.success("Project created, keeping content.");
    // Once, as the owner's code is spent.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [confirmed]);

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
        <div className="min-h-0 flex-1 overflow-y-auto px-gutter">
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
          <SidePanelRow label="Content mode" labelId={modeId}>
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              value={mode}
              // A press on the chosen one would choose none: it stays chosen.
              onValueChange={(next) => next && setMode(next as ContentMode)}
              aria-labelledby={modeId}
              disabled={pending || made !== null}
            >
              {[ContentMode.Off, ContentMode.On].map((m) => (
                <ToggleGroupItem
                  key={m}
                  value={m}
                  disabled={m === ContentMode.On && !isOwner}
                  className="px-gutter"
                >
                  {CONTENT_MODE_LABEL[m]}
                </ToggleGroupItem>
              ))}
            </ToggleGroup>
            <p className="text-muted-foreground">
              {mode === ContentMode.Off
                ? "The content its spans carry — prompts, answers, tool inputs and outputs — is dropped as they arrive, and only metadata is kept."
                : "The content its spans carry is kept, sealed under the project's own key, wherever the code asks the SDK to send it; every read is recorded. You confirm it with a code mailed to you."}
              {!isOwner && " Only the organization's owner can make a project that keeps content."}{" "}
              You can change it later in its settings.
            </p>
            {made && flow.step.step === "code" && (
              <CodeField
                id={codeId}
                label={<CodeSentTo email={email} />}
                value={flow.code}
                onType={flow.type}
                disabled={flow.confirming}
              />
            )}
            {made && flow.error && (
              <p className="text-destructive" role="alert">
                {flow.error}
              </p>
            )}
          </SidePanelRow>
        </div>
        <SidePanelFooter>
          {made ? (
            <>
              <Button type="submit" disabled={flow.sending || (flow.step.step === "code" && !flow.canConfirm)}>
                {flow.confirming
                  ? "Saving…"
                  : flow.sending
                    ? "Sending…"
                    : flow.step.step === "code"
                      ? "Keep content"
                      : "Send code again"}
              </Button>
              {/* The project is made; leaving here opens it with content off. */}
              <Button type="button" variant="outline" onClick={() => open(made)}>
                Keep it off
              </Button>
            </>
          ) : (
            <>
              <Button type="submit" disabled={!canSubmit}>
                {pending ? "Creating…" : "Create project"}
              </Button>
              <Button type="button" variant="outline" onClick={onClose}>
                Cancel
              </Button>
            </>
          )}
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
      <FolderPlus className="size-4" />
      New project
    </PageAction>
  );
}
