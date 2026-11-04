import { DataTable, THead, Th, Td } from "@/components/data-table";
import { LocalTime } from "@/components/local-time";
import { NoticeCard } from "@/components/notice-card";
import { PageHeader } from "@/components/page-header";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Badge } from "@/components/ui/badge";
import { fetchModelConnectionsAnswer } from "@/lib/server/entities/model-connections";
import { projectPage } from "@/lib/server/project-page";
import { administers } from "@/lib/roles";

import { ModelRowActions, ModelsActions, PROVIDER_LABEL } from "./_models";

export const metadata = { title: "Models" };

export default async function ModelsPage({ params }: PageProps<"/[organization]/[project]/models">) {
  const { project: slug } = await params;
  const resolved = await projectPage(slug);
  if (resolved.kind === "unavailable") return <ServiceUnavailable />;
  const { project } = resolved;
  const answer = await fetchModelConnectionsAnswer(project.id);
  const canEdit = administers(project.role);
  const federation = answer?.federation ?? null;

  return (
    <>
      <PageHeader
        title="Models"
        action={canEdit ? <ModelsActions projectId={project.id} federation={federation} /> : undefined}
      />
      <section className="grid gap-gutter">
        {/* No heading: the page's only section, as API keys' is. */}
        <p className="text-sm text-muted-foreground">
          The models this project&apos;s evaluators judge with, on its own keys; each key is sealed when saved and
          shown only by its last four characters.
        </p>
        {!answer ? (
          <NoticeCard
            title="This project's models could not be read."
            body="Reload to try again."
          />
        ) : answer.connections.length === 0 ? (
          <NoticeCard
            title="No model is connected."
            body={
              canEdit
                ? "Connect one for this project's evaluators to judge with."
                : "Its owners and admins connect one for its evaluators to judge with."
            }
          />
        ) : (
          <DataTable>
            <THead>
              <Th>Name</Th>
              <Th>Provider</Th>
              <Th>Models</Th>
              <Th>Key</Th>
              <Th>Last used</Th>
              <Th />
            </THead>
            <tbody>
              {answer.connections.map((connection) => (
                <tr key={connection.id}>
                  <Td className="font-medium whitespace-nowrap">
                    <span className="flex items-center gap-gutter">
                      {connection.name}
                      {connection.is_default && <Badge variant="secondary">Default</Badge>}
                    </span>
                    {connection.refused_at && (
                      <span className="block text-xs font-normal whitespace-normal text-destructive">
                        Key refused: its evaluators wait until it is saved with one the provider takes.
                      </span>
                    )}
                  </Td>
                  <Td className="text-xs text-muted-foreground whitespace-nowrap">
                    {PROVIDER_LABEL[connection.provider]}
                  </Td>
                  <Td className="text-xs text-muted-foreground">{connection.models.join(", ")}</Td>
                  <Td className="whitespace-nowrap">
                    {connection.signs_in ? (
                      <span className="text-xs text-muted-foreground">
                        Signs in as {connection.secret_hint}, no key kept
                      </span>
                    ) : connection.secret_hint ? (
                      <Badge variant="secondary">…{connection.secret_hint}</Badge>
                    ) : (
                      <span className="text-xs text-muted-foreground">None</span>
                    )}
                  </Td>
                  <Td className="text-xs text-muted-foreground whitespace-nowrap">
                    {connection.last_used_at ? <LocalTime iso={connection.last_used_at} mode="date" /> : "Never"}
                  </Td>
                  <Td className="text-right whitespace-nowrap">
                    {canEdit && (
                      <ModelRowActions projectId={project.id} connection={connection} federation={federation} />
                    )}
                  </Td>
                </tr>
              ))}
            </tbody>
          </DataTable>
        )}
      </section>
    </>
  );
}
