import Link from "next/link";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { CONTENT_MODE_LABEL, CONTENT_MODE_MEANS } from "@/lib/content-mode";
import { administers } from "@/lib/roles";
import { fetchContentMode, fetchFieldInventory } from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";

import { EraseEndUserForm } from "./_erase-end-user";

export const metadata = { title: "Privacy" };

// What this project keeps, on one page: its content mode, and the field
// inventory — every attribute ingest reads and what becomes of it — served by
// the server from the code ingest runs, so this page cannot promise what the
// door does not do. A project admin erases an end user here too. The posture
// page grows from here.
export default async function PrivacyPage({
  params,
}: PageProps<"/[organization]/[project]/privacy">) {
  const { organization, project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const [contentMode, inventory] = await Promise.all([
    fetchContentMode(project.id),
    fetchFieldInventory(project.id),
  ]);

  return (
    <>
      <PageHeader title="Privacy" />
      <div className="grid gap-6">
        <Section
          title="Content"
          description="What this project keeps of what a person or a model wrote: prompts and completions, tool inputs and outputs, a person's name or address. Metadata — timings, statuses, counts, model and tool names — is kept in every mode."
        >
          <Card className="grid gap-1 text-sm">
            {contentMode ? (
              <>
                <p>
                  <span className="font-medium">Content mode:</span>{" "}
                  {CONTENT_MODE_LABEL[contentMode.content_mode]}
                </p>
                <p className="text-muted-foreground">
                  {CONTENT_MODE_MEANS[contentMode.content_mode]} The organization owner changes
                  it on{" "}
                  <Link
                    href={projectPath(organization, slug, "/settings")}
                    className="underline underline-offset-4"
                  >
                    Settings
                  </Link>
                  .
                </p>
              </>
            ) : (
              <p className="text-muted-foreground">
                This project&apos;s content mode could not be read. Reload to try again.
              </p>
            )}
          </Card>
        </Section>

        <Section
          title="Erase an end user"
          description={
            <>
              Every trace whose spans carry this id as <code>user.id</code>, the traces of their
              sessions that name no one, and any content they kept, deleted at once. The id is
              matched exactly as your app sent it.{" "}
              <Link href="/docs/workspace/data-deletion" className="underline underline-offset-4">
                What erasure takes
              </Link>
              .
            </>
          }
        >
          <Card className="grid gap-1 text-sm">
            {administers(project.role) ? (
              <EraseEndUserForm projectId={project.id} />
            ) : (
              <p className="text-muted-foreground">
                A project admin or the organization owner erases an end user.
              </p>
            )}
          </Card>
        </Section>

        <Section
          title="Field inventory"
          description={
            <>
              Every attribute Telmoni reads from a span, where it is read, what becomes of it,
              and the content mode it needs. Text under any other name is kept as content where
              the mode is on, and dropped where it is off. The names are the GenAI semantic
              conventions&apos; at a pinned commit; the
              same list is in the{" "}
              <Link href="/docs/api/field-inventory" className="underline underline-offset-4">
                docs
              </Link>
              .
            </>
          }
        >
          {inventory ? (
            <>
              <DataTable>
                <THead>
                  <Th>Attribute</Th>
                  <Th>On</Th>
                  <Th>Kept as</Th>
                  <Th>Needs</Th>
                  <Th>Rule</Th>
                </THead>
                <tbody>
                  {inventory.rows.map((row) => (
                    <tr key={row.attribute}>
                      <Td className="whitespace-nowrap text-xs">{row.attribute}</Td>
                      <Td className="whitespace-nowrap text-xs text-muted-foreground">
                        {row.on}
                      </Td>
                      <Td className="min-w-64 text-xs">{row.kept}</Td>
                      <Td className="whitespace-nowrap text-xs text-muted-foreground">
                        {row.needs}
                      </Td>
                      <Td className="min-w-64 text-xs text-muted-foreground">{row.rule}</Td>
                    </tr>
                  ))}
                </tbody>
              </DataTable>
              <p className="text-xs text-muted-foreground">
                Conventions at commit <code>{inventory.conventions_commit}</code>.
              </p>
            </>
          ) : (
            <Card className="grid gap-1 text-sm">
              <p className="font-medium">The inventory could not be read.</p>
              <p className="text-muted-foreground">
                Reload to try again. The docs page lists the same attributes.
              </p>
            </Card>
          )}
        </Section>
      </div>
    </>
  );
}
