const RESOURCE_LABELS: Record<string, string> = {
  token: "API token",
  ledger_entry: "Ledger entry",
};

export function resourceLabel(kind: string): string {
  return RESOURCE_LABELS[kind] ?? kind.replace(/_/g, " ");
}
