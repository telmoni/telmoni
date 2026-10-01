"use client";

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

import { useCopyToClipboard } from "@/lib/use-copy-to-clipboard";

export function PlaintextTokenDialog({
  token,
  title,
  description,
  closeLabel = "Done",
  onClose,
}: {
  token: string;
  title: string;
  description: React.ReactNode;
  closeLabel?: string;
  onClose: () => void;
}) {
  const { copied, copy } = useCopyToClipboard();

  return (
    <Dialog open onOpenChange={(next) => { if (!next) onClose(); }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogDescription>{description}</DialogDescription>
          <pre className="break-all rounded-md bg-muted p-3 font-mono text-xs select-all">
            {token}
          </pre>
        </DialogBody>
        <DialogFooter>
          <Button variant="secondary" onClick={() => copy(token)}>
            {copied ? "Copied!" : "Copy"}
          </Button>
          <Button onClick={onClose}>{closeLabel}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
