import Link from "next/link";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { CONTENT_MODE_LABEL, CONTENT_MODE_MEANS } from "@/lib/content-mode";
import {
  fetchContentMode,
  fetchFieldInventory,
  fetchProjectBySlug,
  getServerContext,
} from "@/lib/server/data";
import { projectPath } from "@/lib/slug";

export const metadata = { title: "Privacy" };

// What this project keeps, on one page: its content mode, and the field
// inventory — every attribute ingest reads and what becomes of it — served by
// the server from the code ingest runs, so this page cannot promise what the
// door does not do. The posture page grows from here.
export default async function PrivacyPage({
  params,
}: {
  params: Promise<{ organization: string; project: string }>;
}) {
  const { organization, project: slug } = await params;
  const [gate, project] = await Promise.all([getServerContext(), fetchProjectBySlug(slug)]);
  if (!gate || !project) return <ServiceUnavailable />;
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
          title="Field inventory"
          description={
            <>
              Every attribute Telmoni reads from a span, where it is read, what becomes of it,
              and the content mode it needs. Text under any other name is dropped in every
              mode. The names are the GenAI semantic conventions&apos; at a pinned commit; the
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
                      {/* The three rows that name no attribute say what they
                          cover in words, and read as words. */}
                      <Td
                        className={
                          row.attribute.includes(".")
                            ? "whitespace-nowrap font-mono text-xs"
                            : "whitespace-nowrap text-xs"
                        }
                      >
                        {row.attribute}
                      </Td>
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
