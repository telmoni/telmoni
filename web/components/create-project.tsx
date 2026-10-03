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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { createProjectAction } from "@/app/(app)/actions";
import { organizationLabel } from "@/lib/identity";
import { projectPath } from "@/lib/slug";
import {
  useActiveOrganizationId,
  useAddProject,
  useOrganizations,
  useRoles,
} from "@/lib/store";
import { Role } from "@/lib/types/enums";

const MAX_PROJECT_NAME = 100;

/// Whether the caller may create a project in the organization they are STANDING
/// IN. This is the page action's question — "New project" on an organization's
/// own Projects page is about that organization.
export function useCanCreateProjects(): boolean {
  const role = useRoles().organization;
  return role === Role.Owner || role === Role.Admin;
}

/// An organization the caller may create a project in.
export interface CreateProjectTarget {
  id: string;
  label: string;
}

/// Every organization the caller may create a project in — the ones they own,
/// then the ones they administer.
///
/// ⚠ **This is a different question from [`useCanCreateProjects`], and conflating
/// them is what put the wrong organization's name on the switcher's Create
/// project row.** That row offered "In <the active organization>", which is an
/// assertion the console is in no position to make: somebody may administer
/// three organizations and be standing in a fourth they only hold a seat in.
/// The row says "Create project" and the dialog asks.
export function useCreateProjectTargets(): CreateProjectTarget[] {
  const organizations = useOrganizations();
  const rank = (role: string) => (role === "owner" ? 0 : 1);
  return organizations
    .filter((o) => o.role === "owner" || o.role === "admin")
    .sort((a, b) => rank(a.role) - rank(b.role))
    // The organization's name, never a person's name or address: this dialog
    // asks which organization the project lands in.
    .map((o) => ({ id: o.organizationId, label: organizationLabel(o) }));
}

export function CreateProjectDialog({
  open,
  onOpenChange,
  organizationId,
}: {
  open: boolean;
  onOpenChange: (v: boolean) => void;
  /// Preselect, for an entry point that already names an organization — the
  /// "New project" action on an organization's own Projects page. The switcher
  /// passes nothing, because it is not standing anywhere in particular.
  organizationId?: string;
}) {
  const router = useRouter();
  const addProject = useAddProject();
  const organizations = useOrganizations();
  const activeOrganizationId = useActiveOrganizationId();
  const targets = useCreateProjectTargets();
  const [name, setName] = useState("");
  const [target, setTarget] = useState(
    () => organizationId ?? targets[0]?.id ?? "",
  );
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const canSubmit = name.trim().length > 0 && target.length > 0 && !pending;
  const chosen = targets.find((t) => t.id === target);

  function submit() {
    setError(null);
    start(async () => {
      try {
        const res = await createProjectAction(name.trim(), target);
        if (res.error) {
          setError(res.error);
          return;
        }
        onOpenChange(false);
        toast.success("Project created.");
        const landed = organizations.find((o) => o.organizationId === target);
        if (res.project && landed) {
          // Only the organization the console stands in is listed in the
          // store; a project made in another arrives with its own seed.
          if (target === activeOrganizationId) addProject(res.project);
          router.push(projectPath(landed.slug, res.project.slug));
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
            {/* ⚠ Only when there is a choice to make. One organization and a
                picker is a decision presented to somebody who has none, and the
                sentence below says where the project lands either way. */}
            {targets.length > 1 ? (
              <div className="grid gap-2">
                <Label htmlFor="project-organization">Organization</Label>
                <Select value={target} onValueChange={setTarget}>
                  <SelectTrigger id="project-organization" className="w-full">
                    <SelectValue placeholder="Choose an organization" />
                  </SelectTrigger>
                  <SelectContent>
                    {targets.map((t) => (
                      <SelectItem key={t.id} value={t.id}>
                        {t.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            ) : (
              chosen && (
                <p className="text-sm text-muted-foreground">
                  This project will belong to{" "}
                  <span className="font-medium text-foreground">
                    {chosen.label}
                  </span>
                  .
                </p>
              )
            )}
            <div className="grid gap-2">
              <Label htmlFor="project-name">Project name</Label>
              <Input
                id="project-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="Platform"
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

export function CreateProjectAction({
  organizationId,
}: {
  organizationId?: string;
} = {}) {
  const [open, setOpen] = useState(false);
  const canCreate = useCanCreateProjects();

  if (!canCreate) return null;

  return (
    <>
      <PageAction primary="New project" onClick={() => setOpen(true)}>
        <Plus className="size-4" />
        New project
      </PageAction>
      <CreateProjectDialog
        key={`project-${open}`}
        open={open}
        onOpenChange={setOpen}
        organizationId={organizationId}
      />
    </>
  );
}
