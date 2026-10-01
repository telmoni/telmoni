"use client";

import { useState } from "react";
import Link from "next/link";
import { X } from "lucide-react";

import type { ProjectNotification } from "@/lib/types/announcement";
export type { ProjectNotification } from "@/lib/types/announcement";

export function ProjectBanner({
  notification,
}: {
  notification?: ProjectNotification | string | null;
}) {
  const [dismissed, setDismissed] = useState(false);

  const envAnnouncement = process.env.NEXT_PUBLIC_TELMONI_ANNOUNCEMENT;
  const activeNotification: ProjectNotification | null = notification
    ? typeof notification === "string"
      ? { message: notification }
      : notification
    : envAnnouncement
      ? { message: envAnnouncement }
      : null;

  const contentKey = activeNotification
    ? `${activeNotification.id ?? ""}:${activeNotification.message}`
    : "";
  // Adjusted during render rather than in an effect: an effect would paint one
  // frame with the previous notification's dismissal still applied, hiding a
  // banner that has just changed. Same shape as confirm-dialog.tsx.
  const [lastKey, setLastKey] = useState(contentKey);
  if (contentKey !== lastKey) {
    setLastKey(contentKey);
    setDismissed(false);
  }

  if (dismissed || !activeNotification || !activeNotification.message.trim()) {
    return null;
  }

  return (
    <div
      role="status"
      className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-primary/20 bg-primary/5 px-4 py-2.5 text-sm"
    >
      <div className="flex items-center gap-2 text-foreground">
        <span className="font-semibold text-primary">Telmoni Project:</span>
        <p>{activeNotification.message}</p>
      </div>

      <div className="flex items-center gap-3 shrink-0">
        {activeNotification.href && activeNotification.linkText && (
          <Link
            href={activeNotification.href}
            className="font-medium text-foreground underline underline-offset-4 hover:text-muted-foreground"
          >
            {activeNotification.linkText}
          </Link>
        )}
        <button
          type="button"
          onClick={() => setDismissed(true)}
          aria-label="Close notification"
          className="text-muted-foreground hover:text-foreground transition-colors p-0.5 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <X className="size-4" />
        </button>
      </div>
    </div>
  );
}
