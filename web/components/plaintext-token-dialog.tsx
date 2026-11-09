"use client";

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
}: {
  title: string;
  note?: React.ReactNode;
  children: React.ReactNode;
}) {
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
export function CopyBox({ value, label, className }: { value: string; label: string; className?: string }) {
  return (
    <div className={cn("flex items-start gap-gutter rounded-md border border-border bg-muted/40 p-gutter", className)}>
      {/* Padded to the copy button's height, so the first line sits on its icon. */}
      <pre className="min-w-0 flex-1 py-[calc((var(--spacing-control)_-_1lh)/2)] text-sm break-all whitespace-pre-wrap select-all">
        {value}
      </pre>
      <CopyButton value={value} label={label} />
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
            <CopyBox value={token} label={`the ${label.toLowerCase()}`} />
          </DialogSection>
          {children}
        </DialogBody>
        <DialogFooter>
          <Button onClick={onClose}>{closeLabel}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
