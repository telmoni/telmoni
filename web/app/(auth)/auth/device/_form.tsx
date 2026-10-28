"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useAction } from "@/lib/use-action";

import { deviceDecisionAction } from "../_accounts/actions";

export function DeviceForm({ code: initialCode }: { code: string }) {
  const [code, setCode] = useState(initialCode);
  const [decided, setDecided] = useState<"approve" | "deny" | null>(null);
  const { pending, error, run } = useAction();

  if (decided === "approve") {
    return <p className="text-sm">Approved. The device is signing in; you can close this window.</p>;
  }
  if (decided === "deny") {
    return <p className="text-sm">Denied. The device was told, and nothing else happens.</p>;
  }

  const decide = (decision: "approve" | "deny") => {
    run(
      () => deviceDecisionAction({ userCode: code.trim(), decision }),
      () => setDecided(decision),
    );
  };

  return (
    <form
      className="grid gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        if (!pending) decide("approve");
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="code">Code</Label>
        <Input
          id="code"
          name="code"
          autoComplete="off"
          autoCapitalize="characters"
          spellCheck={false}
          required
          placeholder="WDJB-MJHT"
          className="tracking-widest"
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
      </div>
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
      <div className="flex gap-2">
        <Button type="submit" disabled={pending || !code.trim()}>
          {pending ? "Working…" : "Approve"}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={pending || !code.trim()}
          onClick={() => decide("deny")}
        >
          Deny
        </Button>
      </div>
    </form>
  );
}
