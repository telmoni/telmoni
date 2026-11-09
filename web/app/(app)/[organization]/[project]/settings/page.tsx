import Link from "next/link";

import { DataTable, THead, Th, Td } from "@/components/data-table";
import { CopyButton } from "@/components/copy-button";
import { PageHeader } from "@/components/page-header";
import { Section } from "@/components/section";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { CAP_FIELDS } from "@/lib/limits";
import { fetchContentMode, fetchModelPrices, fetchPrivacyPosture } from "@/lib/server/data";
import { fetchLimits } from "@/lib/server/entities/limits";
import { formatCount } from "@/lib/traces";
import { projectPage } from "@/lib/server/project-page";
import { administers } from "@/lib/roles";
import { getServerSession } from "@/lib/server/session";
import { projectPath } from "@/lib/slug";
import { Role } from "@/lib/types/enums";

import { ContentModeForm } from "./_content-mode";
import { DeleteProjectForm } from "./_delete-project";
import { EraseEndUserForm } from "./_erase-end-user";
import { ModelPricesForm } from "./_model-prices";
import { RenameProjectForm } from "./_rename-project";
import { RetentionForm } from "./_retention";

export const metadata = {
  title: "Settings",
};

export default async function SettingsPage({
  params,
}: PageProps<"/[organization]/[project]/settings">) {
  const { organization, project: slug } = await params;

  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const [contentMode, prices, posture, session, limits] = await Promise.all([
    fetchContentMode(project.id),
    fetchModelPrices(project.id),
    fetchPrivacyPosture(project.id),
    getServerSession(),
    fetchLimits(),
  ]);
  const caps = limits?.projects.find((share) => share.id === project.id)?.caps;

  // A project's Owner is its organization's: no seat makes anyone else one.
  const isOwner = project.role === Role.Owner;
  const canEdit = administers(project.role);

  return (
    <>
      <PageHeader title="Settings" />

      <div className="grid gap-page">
        <Section card
          title="Project ID"
          description="This project's identifier, which never changes when its name or URL does; the audit log names the project by it."
        >
          {/* The card's title names it; the body is the value alone. */}
          <div className="flex items-center gap-2">
            <code className="min-w-0 break-all">{project.id}</code>
            <CopyButton value={project.id} label="Project ID" />
          </div>
        </Section>

        <Section card
          title="Project name"
          description="Used to identify this project across the console and notifications. Its address follows the name: a rename moves every link to its pages, and a name with no Latin letters or digits keeps the address it has."
        >
          {/* A name changed elsewhere draws a fresh form; the same name keeps what is typed. */}
          <RenameProjectForm
            key={project.name}
            projectId={project.id}
            slug={project.slug}
            initialName={project.name}
            canEdit={canEdit}
          />
        </Section>

        <Section card
          title="Content"
          description="What this project keeps of the content its spans carry: prompts and completions, tool inputs and outputs, error messages, a person's name or email address, and anything else a person or a model wrote. Off keeps none of it: content is dropped as each span arrives, and the span is kept without it. On keeps it as sent, for as long as its span, wherever the code asks the SDK to send it."
        >
          {contentMode ? (
            <ContentModeForm
              key={`${contentMode.content_mode}:${contentMode.offered.join()}`}
              projectId={project.id}
              mode={contentMode.content_mode}
              offered={contentMode.offered}
              canEdit={isOwner}
              email={session?.email ?? ""}
            />
          ) : (
            <p className="text-sm text-muted-foreground">
              This project&apos;s content mode could not be read. Reload to try again.
            </p>
          )}
        </Section>

        <Section card
          title="Model prices"
          description="This project's own prices for the models it calls, which spend reads before the price table's: private pricing, a model it serves itself, or a fine-tune the table does not list. A call that reports its own cost keeps it. Each price is US dollars per million tokens, for a provider's model by every name it answers with, from the day it takes effect, or always."
        >
          {prices ? (
            <ModelPricesForm
              key={JSON.stringify(prices)}
              projectId={project.id}
              prices={prices}
              canEdit={canEdit}
            />
          ) : (
            <p className="text-sm text-muted-foreground">
              This project&apos;s prices could not be read. Reload to try again.
            </p>
          )}
        </Section>

        <Section card
          title="Retention"
          description={
            <>
              How long what this project sends is kept: its spans as long as its owner or an admin
              chooses, up to the longest offered, and its hourly totals the same for every project.
              Deleting a trace, an end user or the project ends it early.{" "}
              <Link href="/docs/workspace/data-retention" className="underline underline-offset-4">
                Every window
              </Link>
              . The organization&apos;s{" "}
              <Link href={`/${organization}/data-sheet`} className="underline underline-offset-4">
                data sheet
              </Link>{" "}
              says this of every project, with what each keeps and who read its content, for a
              security review.
            </>
          }
        >
          {posture ? (
            <DataTable flush>
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
                  <Td className="text-xs">Hourly totals, which name no end user and no tag</Td>
                  <Td className="whitespace-nowrap text-xs">{posture.retention.rollup_months} months</Td>
                </tr>
              </tbody>
            </DataTable>
          ) : (
            <p className="text-muted-foreground">Retention could not be read. Reload to try again.</p>
          )}
          {posture && canEdit && (
            <RetentionForm
              key={posture.retention.span_days}
              projectId={project.id}
              days={posture.retention.span_days}
              max={posture.retention.max_span_days}
            />
          )}
        </Section>

        <Section card
          title="Limits"
          description={
            <>
              What this project may use of the organization&apos;s pool, as its owner and admins
              set it on the organization&apos;s{" "}
              <Link href={`/${organization}/limits`} className="underline underline-offset-4">
                Limits
              </Link>{" "}
              page. A limit with no cap shares the pool with the organization&apos;s other projects.
            </>
          }
        >
          {caps ? (
            <DataTable flush>
              <THead>
                <Th>Limit</Th>
                <Th>Cap</Th>
              </THead>
              <tbody>
                {CAP_FIELDS.map(({ key, label, unit }) => {
                  const cap = caps[key];
                  return (
                    <tr key={key}>
                      <Td className="text-xs">{label}</Td>
                      <Td className="whitespace-nowrap text-xs">
                        {cap === null
                          ? "Shares the pool"
                          : unit
                            ? (cap / unit).toLocaleString("en-US", { maximumFractionDigits: 1 })
                            : formatCount(cap)}
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </DataTable>
          ) : (
            <p className="text-muted-foreground">The limits could not be read. Reload to try again.</p>
          )}
        </Section>

        <Section card
          title="Give an end user their data"
          description="What the project keeps about one end user, for them to have."
        >
          <p>
            Filter{" "}
            <Link href={projectPath(organization, slug, "/traces")} className="underline underline-offset-4">
              Traces
            </Link>{" "}
            by their id under <span className="font-medium">User</span>, then download every trace of
            theirs the project keeps, with any content kept, as JSON or CSV.
          </p>
        </Section>

        <Section card
          title="Erase an end user"
          description={
            <>
              Every trace whose spans carry this id as <code>user.id</code>, the traces of their sessions
              that name no one, and any content they kept, deleted at once. The id is matched exactly as
              your app sent it.{" "}
              <Link href="/docs/workspace/data-deletion" className="underline underline-offset-4">
                What erasure takes
              </Link>
              .
            </>
          }
        >
          <div className="grid gap-1">
            {canEdit ? (
              <EraseEndUserForm projectId={project.id} />
            ) : (
              <p className="text-muted-foreground">A project admin or the organization owner erases an end user.</p>
            )}
          </div>
        </Section>

        {isOwner && (
          <Section card
            title="Danger zone"
            danger
            description="Permanently deletes this project and everything inside it. Immediate and irreversible; no grace period or undo."
          >
            <DeleteProjectForm projectId={project.id} projectName={project.name} />
          </Section>
        )}
      </div>
    </>
  );
}
