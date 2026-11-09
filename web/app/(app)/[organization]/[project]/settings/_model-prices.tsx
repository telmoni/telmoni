"use client";

import { useState } from "react";
import { toast } from "sonner";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import type { ModelPrice } from "@/lib/server/entities/model-prices";
import { useAction } from "@/lib/use-action";
import { decimalTyped } from "@/lib/typed-number";

import { saveModelPricesAction } from "./actions";

/** One row as typed: every field text until it is saved. */
type Draft = {
  provider: string;
  models: string;
  effective_from: string;
  input: string;
  output: string;
  cache_read: string;
  cache_write: string;
};

const PRICES = ["input", "output", "cache_read", "cache_write"] as const;

const LABELS: Record<(typeof PRICES)[number], string> = {
  input: "Input",
  output: "Output",
  cache_read: "Cache read",
  cache_write: "Cache write",
};

function draftOf(price: ModelPrice): Draft {
  return {
    provider: price.provider,
    models: price.models.join(", "),
    effective_from: price.effective_from ?? "",
    input: String(price.input),
    output: String(price.output),
    cache_read: String(price.cache_read),
    cache_write: String(price.cache_write),
  };
}

const EMPTY: Draft = {
  provider: "",
  models: "",
  effective_from: "",
  input: "",
  output: "",
  cache_read: "0",
  cache_write: "0",
};

/** The drafts as the lane takes them, or why they cannot be sent. */
export function pricesOf(drafts: Draft[]): { prices: ModelPrice[] } | { error: string } {
  const prices: ModelPrice[] = [];
  for (const draft of drafts) {
    const models = draft.models
      .split(",")
      .map((model) => model.trim())
      .filter(Boolean);
    const amounts = PRICES.map((key) => Number(draft[key].trim()));
    if (!draft.provider.trim() || models.length === 0) {
      return { error: "Each price names its provider and at least one model." };
    }
    if (PRICES.some((key) => draft[key].trim() === "") || amounts.some((n) => !Number.isFinite(n) || n < 0)) {
      return { error: "Each price is a number of US dollars, 0 or more." };
    }
    const [input, output, cache_read, cache_write] = amounts as [number, number, number, number];
    prices.push({
      provider: draft.provider.trim(),
      models,
      effective_from: draft.effective_from.trim() || null,
      input,
      output,
      cache_read,
      cache_write,
    });
  }
  return { prices };
}

export function ModelPricesForm({
  projectId,
  prices,
  canEdit,
}: {
  projectId: string;
  prices: ModelPrice[];
  canEdit: boolean;
}) {
  const saved = () => prices.map(draftOf);
  const [drafts, setDrafts] = useState<Draft[]>(saved);
  const { pending, error, setError, run } = useAction();
  const changed = JSON.stringify(drafts) !== JSON.stringify(saved());

  const edit = (at: number, change: Partial<Draft>) =>
    setDrafts((held) => held.map((draft, i) => (i === at ? { ...draft, ...change } : draft)));

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!canEdit || pending || !changed) return;
    const asked = pricesOf(drafts);
    if ("error" in asked) {
      setError(asked.error);
      return;
    }
    run(
      () => saveModelPricesAction(projectId, asked),
      () => toast.success("Model prices saved."),
    );
  };

  return (
    <form onSubmit={handleSubmit} className="grid gap-gutter text-sm">
      {drafts.length === 0 && (
        <p className="text-muted-foreground">
          No prices of this project&apos;s own: its calls are priced by the table.
        </p>
      )}
      {drafts.map((draft, at) => (
        <fieldset
          key={at}
          disabled={!canEdit || pending}
          aria-label={`Price ${at + 1}`}
          className="grid gap-gutter rounded-menu border border-border p-gutter sm:grid-cols-2 lg:grid-cols-4"
        >
          <Input
            aria-label="Provider"
            placeholder="Provider, as gen_ai.provider.name"
            value={draft.provider}
            onChange={(e) => edit(at, { provider: e.target.value })}
          />
          <Input
            aria-label="Models"
            placeholder="Models it answers as, comma-separated"
            value={draft.models}
            onChange={(e) => edit(at, { models: e.target.value })}
            className="sm:col-span-2"
          />
          <Input
            aria-label="Takes effect"
            type="date"
            value={draft.effective_from}
            onChange={(e) => edit(at, { effective_from: e.target.value })}
          />
          {PRICES.map((key) => (
            <Input
              key={key}
              aria-label={`${LABELS[key]}, US dollars per million tokens`}
              placeholder={`${LABELS[key]} $ / 1M`}
              inputMode="decimal"
              value={draft[key]}
              onChange={(e) => edit(at, { [key]: decimalTyped(e.target.value) })}
            />
          ))}
          {canEdit && (
            <Button
              type="button"
              variant="ghost"
              onClick={() => setDrafts((held) => held.filter((_, i) => i !== at))}
              className="justify-self-start"
            >
              Remove
            </Button>
          )}
        </fieldset>
      ))}
      {error && <p className="text-destructive">{error}</p>}
      {canEdit ? (
        <div className="flex gap-gutter">
          <Button
            type="button"
            variant="outline"
            disabled={pending}
            onClick={() => setDrafts((held) => [...held, EMPTY])}
          >
            Add a price
          </Button>
          <Button type="submit" variant="filled" disabled={pending || !changed}>
            {pending ? "Saving..." : "Save"}
          </Button>
          {changed && (
            <Button
              type="button"
              variant="outline"
              disabled={pending}
              onClick={() => {
                setDrafts(saved());
                setError(null);
              }}
            >
              Cancel
            </Button>
          )}
        </div>
      ) : (
        <p className="text-muted-foreground">
          Only a project owner or admin can change its prices.
        </p>
      )}
    </form>
  );
}
