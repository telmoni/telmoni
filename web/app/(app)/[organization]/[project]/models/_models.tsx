"use client";

import { useId, useRef, useState } from "react";
import { BrainCircuit } from "lucide-react";
import { toast } from "sonner";

import { ConfirmDialog } from "@/components/confirm-dialog";
import { EnumSelect } from "@/components/enum-select";
import { PageAction } from "@/components/page-action";
import {
  SidePanel,
  SidePanelBody,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
  usePanelOpening,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
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


/** Connect model, the page's action, opening the panel for a new connection. */
export function ModelsActions({
  projectId,
  federation = null,
}: {
  projectId: string;
  federation?: FederationSetup | null;
}) {
  const { opening, open, close, place } = usePanelOpening();
  return (
    <>
      <PageAction primary="Connect model" onClick={(e) => open(e.currentTarget, null)}>
        <BrainCircuit className="size-4" />
        Connect model
      </PageAction>
      {place(
        opening && (
          <ConnectionPanel
            key={opening.at}
            projectId={projectId}
            connection={null}
            federation={federation}
            onClose={close}
            returnFocus={() => opening.returnTo}
          />
        ),
      )}
    </>
  );
}

/** A listed connection's Edit, opening its panel, and Delete. */
export function ModelRowActions({
  projectId,
  connection,
  federation = null,
}: {
  projectId: string;
  connection: ModelConnection;
  federation?: FederationSetup | null;
}) {
  const { opening, open, close, place } = usePanelOpening();
  const { pending, error, run } = useAction();
  return (
    <div className="flex items-center justify-end gap-gutter">
      {error && <span className="text-xs text-destructive">{error}</span>}
      <Button
        variant="outline"
        size="sm"
        aria-label={`Edit ${connection.name}`}
        onClick={(e) => open(e.currentTarget, null)}
      >
        Edit
      </Button>
      <ConfirmDialog
        trigger={
          <Button
            variant="outline"
            size="sm"
            className="text-destructive hover:bg-destructive/10 hover:text-destructive"
            disabled={pending}
            aria-label={`Delete ${connection.name}`}
          >
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
      {place(
        opening && (
          <ConnectionPanel
            key={opening.at}
            projectId={projectId}
            connection={connection}
            federation={federation}
            onClose={close}
            returnFocus={() => opening.returnTo}
          />
        ),
      )}
    </div>
  );
}

/** One connection in the side panel: a new one asks for its key, or whom it
 *  signs in as; an edit keeps the key unless a new one is typed, and a new
 *  endpoint or region takes the key again. Saving asks the first model one
 *  small question first. */
function ConnectionPanel({
  projectId,
  connection,
  federation,
  onClose,
  returnFocus,
}: {
  projectId: string;
  connection: ModelConnection | null;
  federation: FederationSetup | null;
  onClose: () => void;
  returnFocus: () => HTMLElement | null;
}) {
  const id = useId();
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const field = useRef<HTMLInputElement>(null);
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
  // Under an edit's credential: whether leaving it empty keeps what is saved.
  const keeps = connection !== null && !switched && (moved ? "takes it again" : "keeps it");
  const keepNote = (what: string, empty = "it") =>
    keeps && (
      <p className="text-muted-foreground">
        {keeps === "keeps it"
          ? `Leave ${empty} empty to keep the ${what}.`
          : `A new ${asks.region && !asks.base ? (asks.json ? "location" : "region") : "endpoint"} takes the ${what} again.`}
      </p>
    );

  return (
    <SidePanel panel={panel} titleId={titleId} onClose={onClose} full initialFocus={field} returnFocus={returnFocus}>
      <form
        aria-label={connection ? `Edit ${connection.name}` : "Connect a model"}
        className="flex min-h-0 flex-1 flex-col"
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
              onClose();
            },
          );
        }}
      >
        <SidePanelHeader
          icon={BrainCircuit}
          titleId={titleId}
          title={connection ? `Edit ${connection.name}` : "Connect a model"}
          onClose={onClose}
        >
          <p className="text-sm text-muted-foreground">
            A key of this project&apos;s own to a model its evaluators judge with: sealed when saved, bound to
            where it goes, and never shown again but by its last four characters.
          </p>
        </SidePanelHeader>
        <SidePanelBody>
          <SidePanelRow label="Name" htmlFor={`${id}name`}>
            <Input
              ref={field}
              id={`${id}name`}
              value={name}
              onChange={(e) => setName(e.target.value)}
              maxLength={MODEL_CONNECTION_NAME_MAX}
              placeholder="e.g. judge"
              autoComplete="off"
              className="max-w-md"
            />
          </SidePanelRow>
          <SidePanelRow label="Provider" htmlFor={`${id}provider`}>
            <EnumSelect
              id={`${id}provider`}
              value={provider}
              options={Object.values(ModelProvider)}
              label={(each) => PROVIDER_LABEL[each]}
              onChange={setProvider}
              disabled={connection !== null}
            />
          </SidePanelRow>
          {asks.base && (
            <SidePanelRow
              label={provider === ModelProvider.AzureOpenai ? "Resource endpoint" : "Base URL"}
              htmlFor={`${id}base`}
            >
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
                className="max-w-md"
              />
            </SidePanelRow>
          )}
          {asks.region && (
            <SidePanelRow label={provider === ModelProvider.Bedrock ? "Region" : "Location"} htmlFor={`${id}region`}>
              <Input
                id={`${id}region`}
                value={region}
                onChange={(e) => setRegion(e.target.value)}
                placeholder={provider === ModelProvider.Bedrock ? "us-east-1" : "global"}
                spellCheck={false}
                className="max-w-md"
              />
            </SidePanelRow>
          )}
          <SidePanelRow label={provider === ModelProvider.AzureOpenai ? "Deployments" : "Models"} htmlFor={`${id}models`}>
            <Input
              id={`${id}models`}
              value={models}
              onChange={(e) => setModels(e.target.value)}
              spellCheck={false}
              className="max-w-md"
            />
            <p className="text-muted-foreground">By commas; an evaluator judges with one of them.</p>
          </SidePanelRow>
          {federable && (
            <SidePanelRow label="Signs in with" htmlFor={`${id}signs`}>
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
              {asProject && federation && (
                <div className="grid max-w-md gap-2 rounded-md border border-border p-gutter text-xs">
                  <p className="text-muted-foreground">
                    {asks.pair
                      ? "In AWS IAM, add an OpenID Connect provider with this issuer and audience, and a role it may assume whose trust policy requires this subject. The role needs bedrock:InvokeModel on the models above, and nothing else."
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
            </SidePanelRow>
          )}
          {asProject && asks.pair && (
            <SidePanelRow label="Role ARN" htmlFor={`${id}role`}>
              <Input
                id={`${id}role`}
                value={role}
                onChange={(e) => setRole(e.target.value)}
                placeholder="arn:aws:iam::123456789012:role/telmoni-judge"
                spellCheck={false}
                className="max-w-md"
              />
              {keepNote("role")}
            </SidePanelRow>
          )}
          {asProject && asks.json && (
            <>
              <SidePanelRow label="Workload identity provider" htmlFor={`${id}pool`}>
                <Input
                  id={`${id}pool`}
                  value={pool}
                  onChange={(e) => setPool(e.target.value)}
                  placeholder="projects/123456789/locations/global/workloadIdentityPools/telmoni/providers/judge"
                  spellCheck={false}
                  className="max-w-md"
                />
                {keepNote("pool", "all three")}
              </SidePanelRow>
              <SidePanelRow label="Google Cloud project" htmlFor={`${id}cloudProject`}>
                <Input
                  id={`${id}cloudProject`}
                  value={cloudProject}
                  onChange={(e) => setCloudProject(e.target.value)}
                  spellCheck={false}
                  className="max-w-md"
                />
              </SidePanelRow>
              <SidePanelRow label="Service account" htmlFor={`${id}account`}>
                <Input
                  id={`${id}account`}
                  value={account}
                  onChange={(e) => setAccount(e.target.value)}
                  placeholder="judge@your-project.iam.gserviceaccount.com"
                  spellCheck={false}
                  className="max-w-md"
                />
                <p className="text-muted-foreground">Optional: the one to stand in for.</p>
              </SidePanelRow>
            </>
          )}
          {asks.pair && !asProject && (
            <SidePanelRow label="Access key id" htmlFor={`${id}access`}>
              <Input
                id={`${id}access`}
                value={accessKey}
                onChange={(e) => setAccessKey(e.target.value)}
                autoComplete="off"
                spellCheck={false}
                className="max-w-md"
              />
            </SidePanelRow>
          )}
          {!asProject && (
            <SidePanelRow
              label={asks.json ? "Service account key (JSON)" : asks.pair ? "Secret access key" : "API key"}
              htmlFor={`${id}secret`}
            >
              {asks.json ? (
                <textarea
                  id={`${id}secret`}
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                  rows={4}
                  spellCheck={false}
                  autoComplete="off"
                  className="max-w-md rounded-md border border-input bg-transparent p-gutter font-mono text-xs"
                />
              ) : (
                <Input
                  id={`${id}secret`}
                  type="password"
                  value={secret}
                  onChange={(e) => setSecret(e.target.value)}
                  autoComplete="off"
                  className="max-w-md"
                />
              )}
              {keepNote("key")}
            </SidePanelRow>
          )}
          <SidePanelRow label="Default">
            <div className="flex max-w-md items-center justify-between gap-gutter">
              <Label htmlFor={`${id}default`} className="font-normal">
                The project&apos;s default model connection
              </Label>
              <Switch id={`${id}default`} checked={isDefault} onCheckedChange={setDefault} disabled={pending} />
            </div>
          </SidePanelRow>
          {partial && (
            <p className="py-2 text-sm text-muted-foreground">
              Give the provider and the project together, or leave all three empty.
            </p>
          )}
          {error && (
            <p className="py-2 text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </SidePanelBody>
        <SidePanelFooter>
          <Button type="submit" disabled={!ready || pending}>
            {pending ? "Asking the model…" : connection ? "Save" : "Connect"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
