"use client";

import { ChevronDown } from "lucide-react";
import { CopyButton } from "@/components/copy-button";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";

/** One titled part of a secret's dialog, under its own heading. */
export function DialogSection({
  title,
  note,
  children,
  collapsible = false,
}: {
  title: string;
  note?: React.ReactNode;
  children: React.ReactNode;
  collapsible?: boolean;
}) {
  if (collapsible) {
    return (
      <details className="group rounded-md border border-border">
        <summary className="flex cursor-pointer list-none items-center justify-between gap-gutter p-gutter text-sm font-medium hover:bg-muted/40 [&::-webkit-details-marker]:hidden">
          {title}
          <ChevronDown aria-hidden="true" className="size-icon shrink-0 transition-transform group-open:rotate-180 motion-reduce:transition-none" />
        </summary>
        <div className="grid gap-gutter px-gutter pb-gutter">
          {note && <div className="text-sm text-muted-foreground">{note}</div>}
          {children}
        </div>
      </details>
    );
  }
  return (
    <section className="grid gap-gutter">
      <div>
        <h3 className="text-base font-medium">{title}</h3>
        {note && <div className="text-sm text-muted-foreground">{note}</div>}
      </div>
      {children}
    </section>
  );
}

/** A value to copy, in a box of its own, wrapped rather than scrolled, its copy button inside. */
export function CopyBox({ value, label, className, labelVisible = false }: { value: string; label: string; className?: string; labelVisible?: boolean }) {
  return (
    <div className={cn("flex items-start gap-gutter rounded-md border border-border bg-muted/40 p-gutter", labelVisible && "flex-col sm:flex-row", className)}>
      {/* Padded to the copy button's height, so the first line sits on its icon. */}
      <pre className={cn("min-w-0 flex-1 py-[calc((var(--spacing-control)_-_1lh)/2)] text-sm break-all whitespace-pre-wrap select-all", labelVisible && "w-full break-normal [overflow-wrap:anywhere] sm:w-auto")}>
        {value}
      </pre>
      <CopyButton value={value} label={label} labelVisible={labelVisible} />
    </div>
  );
}

export function PlaintextTokenDialog({
  token,
  title,
  description,
  label = "Secret",
  closeLabel = "Done",
  onClose,
  children,
}: {
  token: string;
  title: string;
  description: React.ReactNode;
  /** The secret's heading, and what its copy button names. */
  label?: string;
  closeLabel?: string;
  onClose: () => void;
  /** What the secret is for, each a `DialogSection` under it. */
  children?: React.ReactNode;
}) {
  return (
    <Dialog open onOpenChange={(next) => { if (!next) onClose(); }}>
      <DialogContent className={cn(children && "sm:max-w-2xl")}>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        <DialogBody className="gap-[calc(var(--spacing-gutter)*2)] pb-gutter">
          <DialogSection title={label} note={<DialogDescription>{description}</DialogDescription>}>
            <CopyBox value={token} label={label} labelVisible />
          </DialogSection>
          {children}
        </DialogBody>
        <DialogFooter>
          <Button variant="filled" onClick={onClose}>{closeLabel}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
