import Link from "next/link";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { CONTENT_MODE_LABEL, CONTENT_MODE_MEANS } from "@/lib/content-mode";
import { CONTENT_READ, READS_MAX, contentReaders } from "@/lib/content-reads";
import { RESIDENCY } from "@/lib/extension/residency";
import { memberNames } from "@/lib/identity";
import { administers } from "@/lib/roles";
import {
  activeOrganization,
  fetchAuditEvents,
  fetchContentMode,
  fetchFieldInventory,
  fetchMembers,
  fetchOrganizationMembers,
  fetchPrivacyPosture,
} from "@/lib/server/data";
import { projectPage } from "@/lib/server/project-page";
import { projectPath } from "@/lib/slug";
import { formatCount } from "@/lib/traces";

import { EraseEndUserForm } from "./_erase-end-user";

export const metadata = { title: "Privacy" };

// The project's privacy posture, the page a security review reads: its
// content mode, what its spans arrived with lately by the field inventory's
// rows, who read its content, how long what it sends is kept and where, and
// the inventory itself — every number the server's, from the code ingest and
// the nightly purge run. A project admin erases an end user here too.
export default async function PrivacyPage({
  params,
}: PageProps<"/[organization]/[project]/privacy">) {
  const { organization, project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { gate, project } = resolved;
  const admin = administers(project.role);
  // The organization's roster names its admins, who read every project with
  // no seat on it; only its owner and admins may read it.
  const [contentMode, inventory, posture, roster, everyone] = await Promise.all([
    fetchContentMode(project.id),
    fetchFieldInventory(project.id),
    fetchPrivacyPosture(project.id),
    admin ? fetchMembers(project.id) : null,
    administers(activeOrganization(gate)?.role) ? fetchOrganizationMembers() : null,
  ]);
  // The audit log is its admins' to read, over the posture's own window.
  const reads =
    admin && posture
      ? await fetchAuditEvents(project.id, READS_MAX, {
          ...CONTENT_READ,
          from: posture.since,
        })
      : null;
  const who = memberNames(
    everyone?.kind === "ok" ? everyone.members : null,
    roster?.kind === "ok" ? roster.members : null,
  );
  const needs = new Map(inventory?.rows.map((row) => [row.attribute, row.needs]));

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
          title="What arrived"
          description={
            posture
              ? `Every attribute the spans this project received in the last ${posture.days} days arrived with, by its row of the field inventory below — content dropped at the door among them. Telmoni counts the names alone, never what they held.`
              : "Every attribute this project's spans arrived with lately, by its row of the field inventory below."
          }
        >
          {!posture ? (
            <NoticeCard title="What arrived could not be read." body="Reload to try again." />
          ) : posture.sent.length === 0 ? (
            <NoticeCard
              title="Nothing arrived."
              body={`No spans arrived in the last ${posture.days} days.`}
            />
          ) : (
            <>
              <DataTable>
                <THead>
                  <Th>Attribute</Th>
                  <Th className="text-right">Spans</Th>
                  <Th>Needs</Th>
                </THead>
                <tbody>
                  {posture.sent.map((row) => (
                    <tr key={row.attribute}>
                      <Td className="whitespace-nowrap text-xs">{row.attribute}</Td>
                      <Td className="whitespace-nowrap text-right text-xs tabular-nums">
                        {formatCount(row.spans)}
                      </Td>
                      <Td className="whitespace-nowrap text-xs text-muted-foreground">
                        {needs.get(row.attribute) ?? "—"}
                      </Td>
                    </tr>
                  ))}
                </tbody>
              </DataTable>
              <p className="text-xs text-muted-foreground">
                Of {formatCount(posture.spans)} spans. A field that needs <code>on</code> is kept
                only while the content mode is on.
              </p>
            </>
          )}
        </Section>

        <Section
          title="Who read content"
          description={
            posture
              ? `Each person who opened this project's content in the last ${posture.days} days, from the audit log, where every read is filed before a field is opened. A project moved from another organization leaves its earlier reads in that one's log.`
              : "Each person who opened this project's content lately, from the audit log."
          }
        >
          {!admin ? (
            <NoticeCard
              title="For admins."
              body="A project admin or the organization owner sees who read content."
            />
          ) : reads?.kind !== "ok" ? (
            <NoticeCard title="Who read content could not be read." body="Reload to try again." />
          ) : reads.events.length === 0 ? (
            <NoticeCard
              title="No reads."
              body={`Nobody read this project's content in the last ${posture?.days} days.`}
            />
          ) : (
            <>
              <DataTable>
                <THead>
                  <Th>Reader</Th>
                  <Th className="text-right">Reads</Th>
                  <Th className="text-right">Traces</Th>
                  <Th>Last read</Th>
                </THead>
                <tbody>
                  {contentReaders(reads.events).map((reader) => (
                    <tr key={reader.actorId}>
                      <Td className="text-xs">{who(reader.actorId)}</Td>
                      <Td className="whitespace-nowrap text-right text-xs tabular-nums">
                        {formatCount(reader.reads)}
                      </Td>
                      <Td className="whitespace-nowrap text-right text-xs tabular-nums">
                        {formatCount(reader.traces)}
                      </Td>
                      <Td className="whitespace-nowrap text-xs text-muted-foreground">
                        <LocalTime iso={reader.last} />
                      </Td>
                    </tr>
                  ))}
                </tbody>
              </DataTable>
              {reads.events.length === READS_MAX && (
                <p className="text-xs text-muted-foreground">
                  The latest {formatCount(READS_MAX)} reads; the{" "}
                  <Link
                    href={projectPath(organization, slug, "/audit-log")}
                    className="underline underline-offset-4"
                  >
                    audit log
                  </Link>{" "}
                  holds every one.
                </p>
              )}
            </>
          )}
        </Section>

        <Section
          title="Retention"
          description={
            <>
              How long what this project sends is kept, the same for every project; deleting a
              trace, an end user or the project ends it early.{" "}
              <Link href="/docs/workspace/data-retention" className="underline underline-offset-4">
                Every window
              </Link>
              .
            </>
          }
        >
          {posture ? (
            <DataTable>
              <THead>
                <Th>Data</Th>
                <Th>Kept</Th>
              </THead>
              <tbody>
                <tr>
                  <Td className="text-xs">Spans, and the content kept with them</Td>
                  <Td className="whitespace-nowrap text-xs">
                    {posture.retention.span_days} days after a span arrives
                  </Td>
                </tr>
                <tr>
                  <Td className="text-xs">
                    Hourly totals, which name no end user and no tag
                  </Td>
                  <Td className="whitespace-nowrap text-xs">
                    {posture.retention.rollup_months} months
                  </Td>
                </tr>
              </tbody>
            </DataTable>
          ) : (
            <NoticeCard title="Retention could not be read." body="Reload to try again." />
          )}
        </Section>

        <Section title="Residency" description="Where what this project sends is stored.">
          <Card className="grid gap-1 text-sm">
            {RESIDENCY ? (
              <p>
                {RESIDENCY.where}{" "}
                <Link href={RESIDENCY.href} className="underline underline-offset-4">
                  More
                </Link>
                .
              </p>
            ) : (
              <p>
                In this deployment&apos;s own stores, wherever its operator runs them: spans in
                ClickHouse, and content in Postgres.
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
            {admin ? (
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
            <NoticeCard
              title="The inventory could not be read."
              body="Reload to try again. The docs page lists the same attributes."
            />
          )}
        </Section>
      </div>
    </>
  );
}
