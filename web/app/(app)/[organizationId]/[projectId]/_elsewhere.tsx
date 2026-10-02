"use client";

import { useTransition } from "react";

import { switchActiveOrganizationAction } from "@/app/(app)/actions";

/// A project the person can open, just not from the organization they are
/// working in — most often one that changed hands under an open tab, whose
/// previous owner stays on it as an admin of the new organization's project.
export function ProjectElsewhere({
  projectId,
  projectName,
  organizationId,
  organizationLabel,
}: {
  projectId: string;
  projectName: string;
  organizationId: string;
  organizationLabel: string;
}) {
  const [pending, start] = useTransition();

  return (
    <div className="flex flex-col gap-4 max-w-md py-8" data-testid="project-elsewhere">
      <h1 className="text-xl font-light tracking-tight">
        {projectName} is in {organizationLabel}.
      </h1>
      <p className="text-sm text-muted-foreground leading-relaxed">
        A project belongs to one organization, and this one is not in the
        organization you&apos;re working in. Switch to {organizationLabel} to
        open it.
      </p>
      <div>
        <button
          type="button"
          disabled={pending}
          onClick={() => start(() => switchActiveOrganizationAction(organizationId, projectId))}
          className="text-sm rounded-menu border border-border px-4 py-2.5 hover:bg-secondary transition-colors disabled:opacity-50 cursor-pointer disabled:cursor-not-allowed"
        >
          {pending ? "Switching…" : `Open it in ${organizationLabel}`}
        </button>
      </div>
    </div>
  );
}
