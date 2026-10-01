"use client";

import { useId, useState, type ReactNode } from "react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogBody,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

export function ConfirmDialog({
  trigger,
  title,
  description,
  confirmLabel = "Confirm",
  destructive = false,
  onConfirm,
  open,
  onOpenChange,
  confirmPhrase,
}: {
  trigger?: ReactNode;
  title: string;
  description: ReactNode;
  confirmLabel?: string;
  destructive?: boolean;
  onConfirm: () => void;
  open?: boolean;
  onOpenChange?: (v: boolean) => void;
  confirmPhrase?: string;
}) {
  const [typed, setTyped] = useState("");
  const phraseId = useId();

  const controlled = open !== undefined;
  const [selfOpen, setSelfOpen] = useState(false);
  const isOpen = controlled ? open : selfOpen;
  const handleOpenChange = (next: boolean) => {
    if (!controlled) setSelfOpen(next);
    onOpenChange?.(next);
  };

  const [prevOpen, setPrevOpen] = useState(isOpen);
  if (isOpen !== prevOpen) {
    setPrevOpen(isOpen);
    setTyped("");
  }

  const unlocked = !confirmPhrase || typed.trim() === confirmPhrase.trim();

  return (
    <AlertDialog open={isOpen} onOpenChange={handleOpenChange}>
      {trigger && <AlertDialogTrigger asChild>{trigger}</AlertDialogTrigger>}
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
        </AlertDialogHeader>
        <AlertDialogBody>
          <AlertDialogDescription>{description}</AlertDialogDescription>
          {confirmPhrase && (
            <div className="grid gap-2">
              <Label htmlFor={phraseId}>
                Type <span className="font-mono">{confirmPhrase}</span> to confirm
              </Label>
              <Input
                id={phraseId}
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
                placeholder={confirmPhrase}
                autoComplete="off"
                spellCheck={false}
              />
            </div>
          )}
        </AlertDialogBody>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction
            variant={destructive ? "destructive" : "default"}
            disabled={!unlocked}
            onClick={onConfirm}
          >
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
