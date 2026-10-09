"use client";

import { useRouter } from "next/navigation";
import { Plus } from "lucide-react";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { PageAction } from "@/components/page-action";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { createProjectAction } from "@/app/(app)/actions";
import { projectPath } from "@/lib/slug";
import { useActiveOrganization, useAddProject, useRoles } from "@/lib/store";
import { Role } from "@/lib/types/enums";

const MAX_PROJECT_NAME = 100;

/// Whether the caller may create a project in the organization they are STANDING
/// IN: every entry point's question, since the dialog creates there and asks
/// about no other.
export function useCanCreateProjects(): boolean {
  const role = useRoles().organization;
  return role === Role.Owner || role === Role.Admin;
}

export function CreateProjectDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
}) {
  const router = useRouter();
  const addProject = useAddProject();
  // The project lands in the organization the console stands in, and the
  // dialog asks nothing about it: every way in — the organization's Projects
  // page, and the switcher's project menu, which lists this organization's
  // projects — already stands in one. Whether the caller may create here is
  // the entry point's question (`useCanCreateProjects`).
  const organization = useActiveOrganization();
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
        if (res.error) {
          setError(res.error);
          return;
        }
        onOpenChange(false);
        toast.success("Project created.");
        if (res.project) {
          addProject(res.project);
          router.push(projectPath(organization.slug, res.project.slug));
        } else {
          router.refresh();
        }
      } catch {
        setError("Something went wrong. Please try again.");
      }
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSubmit) submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Create a project</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              A project is what every API resource hangs off, with its own keys
              and members. You can rename it later in its settings.
            </DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor="project-name">Project name</Label>
              <Input
                id="project-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                // An example of what a project is here: the agent it watches.
                placeholder="Support agent"
                maxLength={MAX_PROJECT_NAME}
                autoComplete="off"
                required
              />
            </div>
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {pending ? "Creating…" : "Create project"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function CreateProjectAction() {
  const [open, setOpen] = useState(false);
  const canCreate = useCanCreateProjects();

  if (!canCreate) return null;

  return (
    <>
      <PageAction primary="New project" onClick={() => setOpen(true)}>
        <Plus className="size-4" />
        New project
      </PageAction>
      <CreateProjectDialog key={`project-${open}`} open={open} onOpenChange={setOpen} />
    </>
  );
}
