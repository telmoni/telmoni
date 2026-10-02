"use client";

import { useRouter } from "next/navigation";
import { useState, useTransition } from "react";
import { KeyRound } from "lucide-react";

import { PageAction } from "@/components/page-action";
import { PlaintextTokenDialog } from "@/components/plaintext-token-dialog";
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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { PRODUCT_NAME } from "@/lib/site";

import { mintTokenAction, type MintResult } from "./actions";

export function KeysActions({
  projectId,
  canManage = true,
}: {
  /// The project this page rendered; a key is minted on it or on nothing.
  projectId: string;
  canManage?: boolean;
}) {
  const [mintOpen, setMintOpen] = useState(false);
  const [minted, setMinted] = useState<MintResult | null>(null);

  if (!canManage) return null;

  return (
    <div className="flex items-center gap-1">
      <PageAction primary="Mint key" onClick={() => setMintOpen(true)}>
        <KeyRound className="size-4" />
        Mint key
      </PageAction>

      <MintDialog
        projectId={projectId}
        key={`mint-${mintOpen}`}
        open={mintOpen}
        onOpenChange={setMintOpen}
        onMinted={setMinted}
      />

      {minted?.token && (
        <PlaintextTokenDialog
          token={minted.token}
          title="Key created"
          description={
            <>
              Copy this key now: <strong>it will not be shown again</strong>.{" "}
              {PRODUCT_NAME} never stores the plaintext.
            </>
          }
          closeLabel="I've saved it"
          onClose={() => setMinted(null)}
        />
      )}
    </div>
  );
}

function MintDialog({
  projectId,
  open,
  onOpenChange,
  onMinted,
}: {
  projectId: string;
  open: boolean;
  onOpenChange: (v: boolean) => void;
  onMinted: (r: MintResult) => void;
}) {
  const router = useRouter();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [ttl, setTtl] = useState("90d");
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();

  const canSubmit = name.trim().length > 0 && !pending;

  function submit() {
    setError(null);
    start(async () => {
      const fd = new FormData();
      fd.set("name", name);
      fd.set("description", description);
      fd.set("ttl", ttl);
      fd.set("projectId", projectId);
      try {
        const r = await mintTokenAction(undefined, fd);
        if (r.status === "error") {
          setError(r.error ?? "Something went wrong. Try again.");
          return;
        }
        onOpenChange(false);
        onMinted(r);
        router.refresh();
      } catch {
        setError("Network error. Try again.");
      }
    });
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSubmit) submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Mint a new key</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              A <code>telmoni_</code> token for the API — it acts on behalf of this project. The plaintext
              is shown exactly once.
            </DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor="key-name">Name</Label>
              <Input
                id="key-name"
                value={name}
                onChange={(e) => {
                  setName(e.target.value);
                  setError(null);
                }}
                maxLength={100}
                placeholder="e.g. ci-deploy"
                disabled={pending}
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="key-description">Description (optional)</Label>
              <Input
                id="key-description"
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                maxLength={500}
                placeholder="What is this key for?"
                disabled={pending}
              />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="key-ttl">Expires</Label>
              <Select value={ttl} onValueChange={setTtl} disabled={pending}>
                <SelectTrigger id="key-ttl" className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="30d">30 days</SelectItem>
                  <SelectItem value="90d">90 days</SelectItem>
                  <SelectItem value="1y">1 year</SelectItem>
                  <SelectItem value="never">Never (not recommended)</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {pending ? "Minting…" : "Mint key"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
