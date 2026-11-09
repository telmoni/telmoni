"use client";

import { BookOpen } from "lucide-react";

import { PageAction } from "@/components/page-action";
import { DOCS_URL } from "@/lib/site";

/** The Agents page's way on, another agent: its setup, in a tab of its own.
 *  A client file, so the link `PageAction` takes in its place is an element
 *  here rather than one the server sent. */
export function ConnectAgentAction() {
  return (
    <PageAction primary="Connect an agent" asChild>
      <a href={`${DOCS_URL}/getting-started/connect-an-agent`} target="_blank" rel="noreferrer">
        <BookOpen className="size-icon" />
        Connect an agent
      </a>
    </PageAction>
  );
}
