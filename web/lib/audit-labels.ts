import type { ResourceKind } from "@/lib/types/enums";

const RESOURCE_LABELS: Partial<Record<ResourceKind, string>> = {
  token: "API key",
};

export function resourceLabel(kind: ResourceKind): string {
  return RESOURCE_LABELS[kind] ?? kind.replace(/_/g, " ");
}
