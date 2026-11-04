"use client";

import { useId, useState } from "react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EnumSelect } from "@/components/enum-select";
import { LocalTime } from "@/components/local-time";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { MODEL_CONNECTION_NAME_MAX } from "@/lib/evaluators";
import type { FederationSetup, ModelConnection } from "@/lib/server/entities/model-connections";
import { ModelProvider } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";

import { deleteModelConnectionAction, saveModelConnectionAction } from "./actions";

/** Each provider as the form names it. */
export const PROVIDER_LABEL: Record<ModelProvider, string> = {
  [ModelProvider.Openai]: "OpenAI",
  [ModelProvider.Anthropic]: "Anthropic",
  [ModelProvider.GoogleAiStudio]: "Google AI Studio",
  [ModelProvider.VertexAi]: "Google Vertex AI",
  [ModelProvider.AzureOpenai]: "Azure OpenAI",
  [ModelProvider.Bedrock]: "Amazon Bedrock",
  [ModelProvider.OpenaiCompatible]: "OpenAI-compatible endpoint",
  [ModelProvider.Typesafe]: "TypeSafe (Jev), typed evaluators",
  [ModelProvider.SystemOneCompatible]: "System One endpoint (open-weight), typed evaluators",
};

/** An endpoint of the customer's own, which may ask for no key. */
const OWN_ENDPOINT: ReadonlySet<ModelProvider> = new Set([ModelProvider.OpenaiCompatible, ModelProvider.SystemOneCompatible]);

/** What each provider asks for beside its models. */
function needs(provider: ModelProvider) {
  return {
    base: provider === ModelProvider.AzureOpenai || OWN_ENDPOINT.has(provider),
    region: provider === ModelProvider.Bedrock || provider === ModelProvider.VertexAi,
    pair: provider === ModelProvider.Bedrock,
    json: provider === ModelProvider.VertexAi,
  };
}

/** How a Bedrock or Vertex AI connection signs in: with a key it keeps, or as
 *  the project, keeping none. */
type SignsIn = "key" | "project";

/** The secret a connection that keeps no key saves: whom it signs in as. */
export function federatedSecret(
  provider: ModelProvider,
  fields: { role: string; pool: string; cloudProject: string; account: string },
): string {
  if (provider === ModelProvider.Bedrock) {
    return fields.role.trim() ? JSON.stringify({ role_arn: fields.role.trim() }) : "";
  }
  if (!fields.pool.trim() || !fields.cloudProject.trim()) return "";
  return JSON.stringify({
    workload_identity_provider: fields.pool.trim(),
    project_id: fields.cloudProject.trim(),
    ...(fields.account.trim() ? { service_account: fields.account.trim() } : {}),
  });
}

/** The models a connection names, as typed: by commas or lines. */
export function modelsOf(typed: string): string[] {
  return [...new Set(typed.split(/[\n,]/).map((m) => m.trim()).filter(Boolean))];
}

/** A project's model connections, each its provider, its models and its key's
 *  last four, with Edit and Delete to its owners and admins, and the form that
 *  connects one; every member reads them. */
export function ModelConnections({
  projectId,
  connections,
  federation = null,
  canEdit,
}: {
  projectId: string;
  connections: readonly ModelConnection[];
  federation?: FederationSetup | null;
  canEdit: boolean;
}) {
  const [editing, setEditing] = useState<ModelConnection | "new" | null>(null);
  const { pending, run } = useAction();
  return (
    <div className="grid gap-gutter text-sm">
      {connections.length === 0 && (
        <p className="text-muted-foreground">
          No model is connected. Evaluators judge with a model on this project&apos;s own key.
        </p>
      )}
      {connections.length > 0 && (
        <ul className="grid gap-gutter">
          {connections.map((connection) => (
            <li key={connection.id} className="flex flex-wrap items-center justify-between gap-gutter">
              <div className="grid min-w-0">
                <span className="flex items-center gap-gutter font-medium">
                  {connection.name}
                  {connection.is_default && <Badge variant="secondary">Default</Badge>}
                </span>
                <span className="truncate text-muted-foreground">
                  {PROVIDER_LABEL[connection.provider]} · {connection.models.join(", ")}
                  {connection.signs_in ? (
                    <> · signs in as {connection.secret_hint}, no key kept</>
                  ) : (
                    connection.secret_hint && <> · key …{connection.secret_hint}</>
                  )}
                </span>
                <span className="text-muted-foreground">
                  {connection.last_used_at ? (
                    <>
                      Last used <LocalTime iso={connection.last_used_at} />
                    </>
                  ) : (
                    "Never used"
                  )}
                </span>
                {connection.refused_at && (
                  <span className="text-destructive">
                    Its key was refused, so its evaluators are paused: save it again with a key the provider takes.
                  </span>
                )}
              </div>
              {canEdit && (
                <div className="flex gap-gutter">
                  <Button variant="outline" size="sm" onClick={() => setEditing(connection)}>
                    Edit
                  </Button>
                  <ConfirmDialog
                    trigger={
                      <Button variant="outline" size="sm" disabled={pending}>
                        Delete
                      </Button>
                    }
                    title={`Delete ${connection.name}?`}
                    description="Its key is deleted with it, and its evaluators judge no more until they name another connection."
                    confirmLabel="Delete"
                    destructive
                    onConfirm={() =>
                      run(
                        () => deleteModelConnectionAction(projectId, connection.id),
                        () => toast.success(`${connection.name} deleted`),
                      )
                    }
                  />
                </div>
              )}
            </li>
          ))}
        </ul>
      )}
      {canEdit &&
        (editing ? (
          <ConnectionForm
            key={editing === "new" ? "new" : editing.id}
            projectId={projectId}
            connection={editing === "new" ? null : editing}
            federation={federation}
            onDone={() => setEditing(null)}
          />
        ) : (
          <div>
            <Button variant="outline" size="sm" onClick={() => setEditing("new")}>
              Connect a model
            </Button>
          </div>
        ))}
    </div>
  );
}

/** One connection's form: a new one asks for its key, or whom it signs in
 *  as; an edit keeps the key unless a new one is typed, and a new endpoint or
 *  region takes the key again. Saving asks the first model one small question first. */
function ConnectionForm({
  projectId,
  connection,
  federation,
  onDone,
}: {
  projectId: string;
  connection: ModelConnection | null;
  federation: FederationSetup | null;
  onDone: () => void;
}) {
  const id = useId();
  const { pending, error, run } = useAction();
  const [name, setName] = useState(connection?.name ?? "");
  const [provider, setProvider] = useState<ModelProvider>(connection?.provider ?? ModelProvider.Openai);
  const [base, setBase] = useState(connection?.base_url ?? "");
  const [region, setRegion] = useState(connection?.region ?? "");
  const [models, setModels] = useState(connection?.models.join(", ") ?? "");
  const [isDefault, setDefault] = useState(connection?.is_default ?? false);
  const [secret, setSecret] = useState("");
  const [accessKey, setAccessKey] = useState("");
  const [signsIn, setSignsIn] = useState<SignsIn>(connection?.signs_in ? "project" : "key");
  const [role, setRole] = useState("");
  const [pool, setPool] = useState("");
  const [cloudProject, setCloudProject] = useState("");
  const [account, setAccount] = useState("");
  const asks = needs(provider);
  const federable = federation !== null && (asks.pair || asks.json);
  const asProject = federable && signsIn === "project";
  const typedSecret = asProject
    ? federatedSecret(provider, { role, pool, cloudProject, account })
    : asks.pair
      ? accessKey.trim() && secret.trim()
        ? JSON.stringify({ access_key_id: accessKey.trim(), secret_access_key: secret.trim() })
        : ""
      : secret;
  // The key goes where the connection points, so a new endpoint takes it again.
  const moved =
    connection !== null &&
    ((asks.base && base.trim().replace(/\/+$/, "") !== (connection.base_url ?? "")) ||
      (asks.region && region.trim() !== (connection.region ?? "")));
  // A connection switched between a key and a sign-in keeps neither: it takes the new one.
  const switched = connection !== null && federable && asProject !== connection.signs_in;
  const keyNeeded = ((connection === null || moved) && !OWN_ENDPOINT.has(provider)) || switched;
  // A pool is its provider and its project together: half of one is no change to keep.
  const partial = asProject && asks.json && !typedSecret && [pool, cloudProject, account].some((field) => field.trim());
  const ready = name.trim() && modelsOf(models).length > 0 && (!keyNeeded || typedSecret) && !partial;
  return (
    <form
      aria-label={connection ? `Edit ${connection.name}` : "Connect a model"}
      className="grid gap-gutter rounded-md border border-border p-gutter"
      onSubmit={(event) => {
        event.preventDefault();
        if (!ready || pending) return;
        run(
          () =>
            saveModelConnectionAction(projectId, {
              id: connection?.id,
              name: name.trim(),
              provider,
              base_url: asks.base ? base.trim() : undefined,
              region: asks.region && region.trim() ? region.trim() : undefined,
              models: modelsOf(models),
              is_default: isDefault,
              // Sent as typed where one is needed: an endpoint of the customer's own may take none.
              secret: typedSecret || (connection === null || moved ? "" : undefined),
            }),
          () => {
            toast.success(connection ? "Connection saved" : "Model connected");
            onDone();
          },
        );
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor={`${id}name`}>Name</Label>
        <Input id={`${id}name`} value={name} onChange={(e) => setName(e.target.value)} maxLength={MODEL_CONNECTION_NAME_MAX} />
      </div>
      <div className="grid gap-2">
        <Label htmlFor={`${id}provider`}>Provider</Label>
        <EnumSelect
          id={`${id}provider`}
          value={provider}
          options={Object.values(ModelProvider)}
          label={(each) => PROVIDER_LABEL[each]}
          onChange={setProvider}
          disabled={connection !== null}
        />
      </div>
      {asks.base && (
        <div className="grid gap-2">
          <Label htmlFor={`${id}base`}>
            {provider === ModelProvider.AzureOpenai ? "Resource endpoint" : "Base URL"}
          </Label>
          <Input
            id={`${id}base`}
            value={base}
            onChange={(e) => setBase(e.target.value)}
            placeholder={
              provider === ModelProvider.AzureOpenai
                ? "https://your-resource.openai.azure.com"
                : provider === ModelProvider.SystemOneCompatible
                  ? "https://clef.example.com"
                  : "https://gateway.example.com/v1"
            }
            spellCheck={false}
          />
        </div>
      )}
      {asks.region && (
        <div className="grid gap-2">
          <Label htmlFor={`${id}region`}>{provider === ModelProvider.Bedrock ? "Region" : "Location"}</Label>
          <Input
            id={`${id}region`}
            value={region}
            onChange={(e) => setRegion(e.target.value)}
            placeholder={provider === ModelProvider.Bedrock ? "us-east-1" : "global"}
            spellCheck={false}
          />
        </div>
      )}
      <div className="grid gap-2">
        <Label htmlFor={`${id}models`}>
          {provider === ModelProvider.AzureOpenai ? "Deployments" : "Models"}, by commas
        </Label>
        <Input id={`${id}models`} value={models} onChange={(e) => setModels(e.target.value)} spellCheck={false} />
      </div>
      {federable && (
        <div className="grid gap-2">
          <Label htmlFor={`${id}signs`}>Signs in with</Label>
          <EnumSelect
            id={`${id}signs`}
            value={signsIn}
            options={["key", "project"] as const}
            label={(each) =>
              each === "key"
                ? asks.pair
                  ? "An access key pair, kept sealed"
                  : "A service account key, kept sealed"
                : asks.pair
                  ? "An IAM role trusting this project, no key kept"
                  : "A workload identity pool trusting this project, no key kept"
            }
            onChange={setSignsIn}
          />
        </div>
      )}
      {asProject && federation && (
        <div className="grid gap-2 rounded-md border border-border p-gutter text-xs">
          <p className="text-muted-foreground">
            {asks.pair
              ? "In AWS IAM, add an OpenID Connect provider with this issuer and audience, and a role it may assume whose trust policy requires this subject. The role needs bedrock:InvokeModel on the models below, and nothing else."
              : "In Google Cloud, add a workload identity pool provider (OIDC) with this issuer, mapping google.subject to assertion.sub, and grant this subject a role holding aiplatform.endpoints.predict and nothing else, or grant it Workload Identity User on a service account that holds one."}
          </p>
          <dl className="grid gap-1 font-mono">
            <dt className="text-muted-foreground">Issuer</dt>
            <dd className="select-all break-all">{federation.issuer}</dd>
            <dt className="text-muted-foreground">Subject</dt>
            <dd className="select-all break-all">{federation.subject}</dd>
            {asks.pair && (
              <>
                <dt className="text-muted-foreground">Audience</dt>
                <dd className="select-all break-all">{federation.aws_audience}</dd>
              </>
            )}
          </dl>
        </div>
      )}
      {asProject && asks.pair && (
        <div className="grid gap-2">
          <Label htmlFor={`${id}role`}>
            Role ARN{connection && !switched && (moved ? " — a new region takes the role again" : " — leave empty to keep the role")}
          </Label>
          <Input
            id={`${id}role`}
            value={role}
            onChange={(e) => setRole(e.target.value)}
            placeholder="arn:aws:iam::123456789012:role/telmoni-judge"
            spellCheck={false}
          />
        </div>
      )}
      {asProject && asks.json && (
        <>
          <div className="grid gap-2">
            <Label htmlFor={`${id}pool`}>
              Workload identity provider
              {connection && !switched && (moved ? " — a new location takes the pool again" : " — leave all three empty to keep the pool")}
            </Label>
            <Input
              id={`${id}pool`}
              value={pool}
              onChange={(e) => setPool(e.target.value)}
              placeholder="projects/123456789/locations/global/workloadIdentityPools/telmoni/providers/judge"
              spellCheck={false}
            />
          </div>
          <div className="grid gap-2">
            <Label htmlFor={`${id}cloudProject`}>Google Cloud project</Label>
            <Input id={`${id}cloudProject`} value={cloudProject} onChange={(e) => setCloudProject(e.target.value)} spellCheck={false} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor={`${id}account`}>Service account to stand in for (optional)</Label>
            <Input
              id={`${id}account`}
              value={account}
              onChange={(e) => setAccount(e.target.value)}
              placeholder="judge@your-project.iam.gserviceaccount.com"
              spellCheck={false}
            />
          </div>
        </>
      )}
      {asks.pair && !asProject && (
        <div className="grid gap-2">
          <Label htmlFor={`${id}access`}>Access key id</Label>
          <Input id={`${id}access`} value={accessKey} onChange={(e) => setAccessKey(e.target.value)} autoComplete="off" spellCheck={false} />
        </div>
      )}
      {!asProject && (
        <div className="grid gap-2">
          <Label htmlFor={`${id}secret`}>
            {asks.json ? "Service account key (JSON)" : asks.pair ? "Secret access key" : "API key"}
            {connection && !switched && (moved ? " — a new endpoint takes the key again" : " — leave empty to keep the key")}
          </Label>
          {asks.json ? (
            <textarea
              id={`${id}secret`}
              value={secret}
              onChange={(e) => setSecret(e.target.value)}
              rows={4}
              spellCheck={false}
              autoComplete="off"
              className="rounded-md border border-input bg-transparent p-gutter font-mono text-xs"
            />
          ) : (
            <Input id={`${id}secret`} type="password" value={secret} onChange={(e) => setSecret(e.target.value)} autoComplete="off" />
          )}
        </div>
      )}
      <label className="flex items-center gap-gutter">
        <input type="checkbox" checked={isDefault} onChange={(e) => setDefault(e.target.checked)} />
        The project&apos;s default model connection
      </label>
      {partial && <p className="text-muted-foreground">Give the provider and the project together, or leave all three empty.</p>}
      {error && (
        <p className="text-destructive" role="alert">
          {error}
        </p>
      )}
      <div className="flex gap-gutter">
        <Button type="submit" disabled={!ready || pending}>
          {pending ? "Asking the model…" : connection ? "Save" : "Connect"}
        </Button>
        <Button type="button" variant="outline" onClick={onDone}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
