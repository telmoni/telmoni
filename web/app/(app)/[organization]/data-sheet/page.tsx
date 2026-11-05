import type { ReactNode } from "react";
import Link from "next/link";
import { notFound } from "next/navigation";

import { AccessDenied } from "@/components/access-denied";
import { DataTable, THead, Td, Th } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import {
  EVERY_CHANGE_SINCE,
  PERSON_LABEL,
  READS_DAYS,
  type ModeChange,
  type Reader,
  modeChanges,
  readerName,
  readersByProject,
  scrubbedSpans,
} from "@/lib/data-sheet";
import { RESIDENCY } from "@/lib/extension/residency";
import { memberNames, ownerContact } from "@/lib/identity";
import { SCRUBBED_KIND } from "@/lib/messages";
import { administers } from "@/lib/roles";
import {
  activeOrganization,
  drawDataSheet,
  fetchAuditEvents,
  fetchContentMode,
  fetchFieldInventory,
  fetchOrganizationAuditSince,
  fetchOrganizationMembers,
  fetchPrivacyPosture,
  fetchProjectListing,
  getServerContext,
  identityContext,
  type FieldInventory,
  type PrivacyPosture,
} from "@/lib/server/data";
import { organizationPath, projectPath } from "@/lib/slug";
import { ResourceKind } from "@/lib/types/enums";

import { PrintSheet } from "./_print";

export const metadata = { title: "Data sheet" };

/** The kinds the SDKs' scrubbers replace, by family, as their markers name them. */
const SCRUBBED_BY = (family: "secret" | "pii") =>
  Object.entries(SCRUBBED_KIND)
    .filter(([key]) => key.startsWith(`${family}:`))
    .map(([, name]) => name);

// The page an owner hands a security team in place of a meeting: for each
// project, its content mode and every change of it, its retention, its own
// keys, what it keeps in its mode and who read its content; then what the
// SDKs' scrubbers catch, the deployment's subprocessors, and the audit chain's
// head it was drawn at, its own read, which the book's script checks against
// an export. Drawn from the stores when asked, owners and admins alone, and
// printed by the browser.
export default async function DataSheetPage() {
  const [gate, ident] = await Promise.all([getServerContext(), identityContext()]);
  if (!gate) return <ServiceUnavailable />;
  if (gate.organizationNotFound) notFound();
  const organization = activeOrganization(gate);
  const overview = organization ? organizationPath(organization.slug) : "/console";
  const denied = (
    <AccessDenied
      what="this organization's data sheet"
      owner={ownerContact(organization)}
      backHref={overview}
    />
  );
  if (!organization || !ident || !administers(ident.role)) return denied;

  const [drawn, listing, reads, everyone] = await Promise.all([
    drawDataSheet(),
    fetchProjectListing(),
    fetchOrganizationAuditSince({ action: "viewed", resourceKind: ResourceKind.Project, days: READS_DAYS }),
    fetchOrganizationMembers(),
  ]);
  if (drawn.kind === "forbidden" || reads.kind === "forbidden") return denied;
  if (drawn.kind !== "ok" || listing.kind !== "ok" || reads.kind !== "ok") return <ServiceUnavailable />;

  const projects = await Promise.all(
    listing.projects.map(async (project) => {
      const [mode, posture, history] = await Promise.all([
        fetchContentMode(project.id),
        fetchPrivacyPosture(project.id),
        fetchAuditEvents(project.id, 1000, {
          action: "updated",
          resourceKind: ResourceKind.Project,
          from: EVERY_CHANGE_SINCE,
        }),
      ]);
      return {
        project,
        mode: mode?.content_mode ?? null,
        posture,
        changes: history.kind === "ok" ? modeChanges(history.events) : null,
      };
    }),
  );
  const first = listing.projects[0];
  const inventory = first ? await fetchFieldInventory(first.id) : null;
  const readers = readersByProject(reads.events);
  const name = memberNames(everyone.kind === "ok" ? everyone.members : null, null);

  return (
    <>
      <PageHeader
        title="Data sheet"
        info="What this organization's projects keep, where and under which key, who read their content, and who else processes it, drawn from the stores now and filed in the audit log. Print it for a security review."
        action={<PrintSheet />}
      />
      <div className="grid gap-page" data-print="sheet">
        <section className="grid gap-gutter" aria-labelledby="sheet-drawn">
          <h2 id="sheet-drawn" className="text-sm font-medium">
            {organization.name}, drawn <LocalTime iso={drawn.drawn.drawnAt} /> by {name(ident.userId)}
          </h2>
          <p className="text-sm text-muted-foreground">
            This drawing is row {drawn.drawn.seq} of the organization&apos;s audit log, whose hash is{" "}
            <code className="break-all font-mono text-xs">{drawn.drawn.rowHash}</code>. Export the log
            from that row on and check it with{" "}
            <Link className="underline" href="/docs/workspace/audit-log#exporting-and-verifying">
              the book&apos;s script
            </Link>
            : <code className="font-mono text-xs">node verify-audit.mjs export.json {drawn.drawn.rowHash}</code>.
          </p>
        </section>

        <Section id="sheet-stores" title="Where it is kept, under which key">
          <Facts
            facts={[
              [
                "Spans",
                "Their metadata alone — names, timings, tokens, costs, statuses, tags — in ClickHouse, never a prompt or a reply.",
              ],
              [
                "Content",
                "Kept only by a project whose content mode is on: each field in Postgres, sealed under a key of the project's own, itself wrapped under the deployment's content key (CONTENT_KEK).",
              ],
              [
                "User and session ids",
                "Kept only as the HMAC-SHA-256 of what was sent, under a key of each project's own, sealed under the deployment's hashing key (TELEMETRY_KEK).",
              ],
              ...(RESIDENCY ? [["Where", RESIDENCY.where] as [string, ReactNode]] : []),
            ]}
          />
        </Section>

        <Section id="sheet-projects" title="Each project">
          {projects.length === 0 ? (
            <NoticeCard title="No projects yet." body="Each project this organization makes appears here." />
          ) : (
            projects.map(({ project, mode, posture, changes }) => (
              <Card key={project.id} className="grid gap-gutter break-inside-avoid">
                <h3 className="text-sm font-medium">
                  <Link href={projectPath(organization.slug, project.slug)}>{project.name}</Link>
                </h3>
                <Facts
                  facts={[
                    [
                      "Content mode",
                      mode === null ? "Could not be read" : mode === "on" ? "On: content kept, sealed" : "Off: content dropped at the door",
                    ],
                    ["Retention", retentionWords(posture)],
                    [
                      "Its keys",
                      "Its own hashing key, for user and session ids; and, while its content is on, its own content key.",
                    ],
                    ["Scrubbed", scrubbedWords(posture)],
                  ]}
                />
                <ModeHistory changes={changes} name={name} />
                <Readers readers={readers.get(project.id) ?? []} complete={reads.complete} name={name} />
              </Card>
            ))
          )}
        </Section>

        <Section id="sheet-inventory" title="What a project keeps">
          <Inventory inventory={inventory} />
        </Section>

        <Section id="sheet-scrubbers" title="What the SDKs scrub before content leaves">
          <Facts
            facts={[
              [
                "Secrets",
                `On wherever content is sent, off only where the developer sets TELMONI_SCRUB_SECRETS=0: ${SCRUBBED_BY("secret").join(", ")}.`,
              ],
              [
                "Personal data",
                `Off in the SDKs unless TELMONI_SCRUB_PII=1, on in the coding-agent plugins: ${SCRUBBED_BY("pii").join(", ")}.`,
              ],
              [
                "Not caught",
                "Names, street addresses, text inside an image, and a secret of a shape no rule knows: the mask, a function of the application's own, is for what it alone knows.",
              ],
            ]}
          />
        </Section>

        <Section id="sheet-subprocessors" title="Subprocessors">
          {RESIDENCY && RESIDENCY.subprocessors.length > 0 ? (
            <DataTable>
              <THead>
                <Th>Provider</Th>
                <Th>Purpose</Th>
                <Th>Where</Th>
              </THead>
              <tbody>
                {RESIDENCY.subprocessors.map((one) => (
                  <tr key={`${one.provider}:${one.purpose}`}>
                    <Td>{one.provider}</Td>
                    <Td className="whitespace-normal">{one.purpose}</Td>
                    <Td className="whitespace-normal">{one.location}</Td>
                  </tr>
                ))}
              </tbody>
            </DataTable>
          ) : (
            <p className="text-sm text-muted-foreground">
              This deployment names none: a self-hosted deployment&apos;s stores and processors are its
              operator&apos;s own.
            </p>
          )}
        </Section>
      </div>
    </>
  );
}

function Section({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <section className="grid gap-gutter" aria-labelledby={id}>
      <h2 id={id} className="text-sm font-medium">
        {title}
      </h2>
      {children}
    </section>
  );
}

/** Facts, each named beside it. */
function Facts({ facts }: { facts: readonly (readonly [string, ReactNode])[] }) {
  return (
    <dl className="grid gap-x-gutter gap-y-2 text-sm sm:grid-cols-[10rem_minmax(0,1fr)]">
      {facts.map(([label, value]) => (
        <div key={label} className="contents">
          <dt className="text-muted-foreground">{label}</dt>
          <dd className="min-w-0 wrap-break-word">{value}</dd>
        </div>
      ))}
    </dl>
  );
}

function retentionWords(posture: PrivacyPosture | null): string {
  if (!posture) return "Could not be read";
  const { span_days, rollup_months } = posture.retention;
  return `Spans and content ${span_days} days after they arrive; the hourly totals, which name no user and no tag, ${rollup_months} months.`;
}

function scrubbedWords(posture: PrivacyPosture | null): string {
  if (!posture) return "Could not be read";
  const spans = scrubbedSpans(posture);
  return spans === 0
    ? `No span of the last ${posture.days} days arrived with anything scrubbed.`
    : `${spans.toLocaleString("en-US")} of its ${posture.spans.toLocaleString("en-US")} spans of the last ${posture.days} days arrived with something scrubbed by its SDK.`;
}

function ModeHistory({ changes, name }: { changes: ModeChange[] | null; name: (id: string) => string }) {
  if (changes === null) {
    return <p className="text-sm text-muted-foreground">Its content mode&apos;s history could not be read.</p>;
  }
  if (changes.length === 0) {
    return <p className="text-sm text-muted-foreground">Its content mode has never been changed: off since it was made.</p>;
  }
  return (
    <DataTable>
      <THead>
        <Th>Content mode changed</Th>
        <Th>From</Th>
        <Th>To</Th>
        <Th>By</Th>
      </THead>
      <tbody>
        {changes.map((change) => (
          <tr key={`${change.at}:${change.to}`}>
            <Td>
              <LocalTime iso={change.at} />
            </Td>
            <Td>{change.from}</Td>
            <Td>
              {change.to}
              {change.deleted ? ", its content deleted" : ""}
            </Td>
            <Td>{name(change.by)}</Td>
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}

function Readers({
  readers,
  complete,
  name,
}: {
  readers: Reader[];
  complete: boolean;
  name: (id: string) => string;
}) {
  if (readers.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        Nobody read its content in the last {READS_DAYS} days{complete ? "" : " of the reads read"}.
      </p>
    );
  }
  return (
    <DataTable>
      <THead>
        <Th>Read its content, the last {READS_DAYS} days</Th>
        <Th>Reads</Th>
        <Th>Last</Th>
      </THead>
      <tbody>
        {readers.map((reader) => (
          <tr key={reader.who}>
            <Td>{readerName(reader.who, name)}</Td>
            <Td className="tabular-nums">{reader.reads}</Td>
            <Td>
              <LocalTime iso={reader.last} />
            </Td>
          </tr>
        ))}
      </tbody>
    </DataTable>
  );
}

function Inventory({ inventory }: { inventory: FieldInventory | null }) {
  if (!inventory) {
    return <p className="text-sm text-muted-foreground">The field inventory could not be read, or there is no project to read it on.</p>;
  }
  return (
    <>
      <p className="text-sm text-muted-foreground">
        Every attribute ingest reads, the same for every project: those that need content are kept by a project whose
        mode is on, and dropped at the door by one whose mode is off.
      </p>
      <DataTable>
        <THead>
          <Th>Attribute</Th>
          <Th>Kept as</Th>
          <Th>Mode</Th>
          <Th>Can carry a person</Th>
        </THead>
        <tbody>
          {inventory.rows.map((row) => (
            <tr key={row.attribute}>
              <Td className="font-mono text-xs">{row.attribute}</Td>
              <Td className="whitespace-normal">{row.kept}</Td>
              <Td>{row.needs === "on" ? "On" : "Any"}</Td>
              <Td className="whitespace-normal">{PERSON_LABEL[row.person]}</Td>
            </tr>
          ))}
        </tbody>
      </DataTable>
    </>
  );
}
