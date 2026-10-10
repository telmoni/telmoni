import { PageHeader } from "@/components/page-header";
import { Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { SettingsRow } from "@/components/settings-row";
import { ServiceUnavailable } from "@/components/service-unavailable";
import { Card } from "@/components/ui/card";
import { fetchContentMode, fetchProjectBySlug, getServerContext } from "@/lib/server/data";
import { Role } from "@/lib/types/enums";

import { ContentModeForm } from "./_content-mode";
import { DeleteProjectForm } from "./_delete-project";
import { RenameProjectForm } from "./_rename-project";

export const metadata = {
  title: "Settings",
};

export default async function SettingsPage({
  params,
}: {
  params: Promise<{ organization: string; project: string }>;
}) {
  const { organization, project: slug } = await params;

  const [gate, project] = await Promise.all([
    getServerContext(),
    fetchProjectBySlug(slug),
  ]);
  if (!gate || !project) return <ServiceUnavailable />;
  const contentMode = await fetchContentMode(project.id);

  // A project's Owner is its organization's: no seat makes anyone else one.
  const isOwner = project.role === Role.Owner;
  const canEdit = isOwner || project.role === Role.Admin;

  return (
    <>
      <PageHeader title="Settings" />

      <div className="grid gap-6">
        <Section
          title="Project ID"
          description="Unique identifier for this project used in API requests and SDK configuration."
        >
          <Rows>
            <SettingsRow label="Project ID" mono copy={project.id}>
              {project.id}
            </SettingsRow>
          </Rows>
        </Section>

        <Section
          title="Project name"
          description="Used to identify this project across the console and notifications. Its address follows the name: a rename moves every link to its pages, and a name with no Latin letters or digits keeps the address it has."
        >
          <Card>
            <RenameProjectForm
              projectId={project.id}
              organization={organization}
              slug={project.slug}
              initialName={project.name}
              canEdit={canEdit}
            />
          </Card>
        </Section>

        <Section
          title="Content"
          description="What this project keeps of the content its spans carry: prompts and completions, tool inputs and outputs, error messages, a person's name or email address, and anything else a person or a model wrote. Off keeps none of it: content is dropped as each span arrives, and the span is kept without it."
        >
          <Card>
            {contentMode ? (
              <ContentModeForm
                projectId={project.id}
                mode={contentMode.content_mode}
                offered={contentMode.offered}
                canEdit={isOwner}
              />
            ) : (
              <p className="text-sm text-muted-foreground">
                This project&apos;s content mode could not be read. Reload to try again.
              </p>
            )}
          </Card>
        </Section>

        {isOwner && (
          <Section
            title="Danger zone"
            danger
            description="Permanently deletes this project and everything inside it. Immediate and irreversible; no grace period or undo."
          >
            <Card className="grid gap-3 bg-destructive/5 text-sm">
              <DeleteProjectForm projectId={project.id} projectName={project.name} />
            </Card>
          </Section>
        )}
      </div>
    </>
  );
}
