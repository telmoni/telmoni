"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { projectPath } from "@/lib/slug";
import { useRenameProject } from "@/lib/store";

import { updateProjectNameAction } from "./actions";

export function RenameProjectForm({
  projectId,
  organization,
  slug,
  initialName,
  canEdit,
}: {
  projectId: string;
  /// The slugs this page's path is spelled with.
  organization: string;
  slug: string;
  initialName: string;
  canEdit: boolean;
}) {
  const router = useRouter();
  const renameProject = useRenameProject();
  const [name, setName] = useState(initialName);
  const [error, setError] = useState<string | null>(null);
  const [pending, startTransition] = useTransition();

  // A rename elsewhere — another tab, or a router.refresh() from the realtime
  // listener — sends a new initialName down. Without this the input keeps the
  // old text while isChanged compares it against the new server value. Only a
  // genuine change resets, so a plain refresh does not discard what is typed.
  const [lastInitial, setLastInitial] = useState(initialName);
  if (initialName !== lastInitial) {
    setLastInitial(initialName);
    setName(initialName);
  }

  const isChanged = name.trim() !== initialName && name.trim().length > 0;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canEdit || !isChanged || pending) return;

    setError(null);
    const trimmed = name.trim();
    startTransition(async () => {
      try {
        const res = await updateProjectNameAction(projectId, trimmed);
        if (res.error) {
          setError(res.error);
          toast.error(res.error);
        } else {
          toast.success("Project name updated.");
          renameProject(projectId, trimmed, res.movedTo ?? slug);
          // Its slug follows its name, and so does this page.
          if (res.movedTo) {
            router.replace(projectPath(organization, res.movedTo, "/settings"));
          }
          router.refresh();
        }
      } catch {
        const msg = "Network error. Please try again.";
        setError(msg);
        toast.error(msg);
      }
    });
  };

  return (
    <form onSubmit={handleSubmit} className="grid gap-3">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          disabled={!canEdit || pending}
          placeholder="Project name"
          maxLength={100}
          className="max-w-md"
          aria-label="Project name"
        />
        {canEdit && (
          <Button
            type="submit"
            disabled={!isChanged || pending}
            variant="default"
          >
            {pending ? "Saving..." : "Save"}
          </Button>
        )}
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
      {!canEdit && (
        <p className="text-sm text-muted-foreground">
          Only the project owner can change the project name.
        </p>
      )}
    </form>
  );
}
