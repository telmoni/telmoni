"use client";

import { useRouter } from "next/navigation";

import { Button } from "@/components/ui/button";

/** Asks for the page again: the one interactive part of a notice a server
 *  component draws. */
export function RetryButton() {
  const router = useRouter();
  return (
    <Button variant="outline" onClick={() => router.refresh()}>
      Try again
    </Button>
  );
}
