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
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

export function InviteSentDialog({
  open,
  onOpenChange,
  email,
  link,
  scope = "project",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  email: string;
  link: string;
  scope?: "project" | "organization";
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Invitation sent</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogDescription>
            We emailed {email} a link. It expires, and it works once. Nothing
            changes on your {scope} until they accept.
          </DialogDescription>
          <div className="grid gap-1.5">
            <Label htmlFor="invite-link">Or send it yourself</Label>
            <Input id="invite-link" readOnly value={link} />
            <p className="text-xs text-muted-foreground">
              Treat it like a password — whoever opens it is offered the role,
              and only the address above can accept.
            </p>
          </div>
        </DialogBody>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>Done</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
