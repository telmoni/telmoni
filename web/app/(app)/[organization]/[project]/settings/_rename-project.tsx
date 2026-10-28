"use client";

import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { MAX_PROJECT_NAME, typedBound } from "@/lib/names";
import { useRenameProject } from "@/lib/store";
import { useAction } from "@/lib/use-action";

import { updateProjectNameAction } from "./actions";

export function RenameProjectForm({
  projectId,
  slug,
  initialName,
  canEdit,
}: {
  projectId: string;
  /// The project's slug, which this page's path is spelled with.
  slug: string;
  initialName: string;
  canEdit: boolean;
}) {
  const renameProject = useRenameProject();
  const [name, setName] = useState(initialName);
  const { pending, error, run } = useAction();

  const isChanged = name.trim() !== initialName && name.trim().length > 0;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canEdit || !isChanged || pending) return;
    const trimmed = name.trim();
    // Only a rename that kept the slug answers here: one that moved it takes
    // the page on to the new path in the action itself.
    run(
      () => updateProjectNameAction(projectId, trimmed),
      () => {
        toast.success("Project name updated.");
        renameProject(projectId, trimmed, slug);
      },
    );
  };

  return (
    <form onSubmit={handleSubmit} className="grid gap-3">
      <div className="flex flex-col gap-3 sm:flex-row sm:items-center">
        <Input
          value={name}
          onChange={(e) => setName(e.target.value)}
          disabled={!canEdit || pending}
          placeholder="Project name"
          maxLength={typedBound(MAX_PROJECT_NAME)}
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
          Only a project owner or admin can change the project name.
        </p>
      )}
    </form>
  );
}
