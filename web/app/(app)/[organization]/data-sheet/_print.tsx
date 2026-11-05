"use client";

import { Printer } from "lucide-react";

import { Button } from "@/components/ui/button";

/** The browser's own print, which the print stylesheet in `globals.css` lays
 *  out: the sheet alone, whole, with no PDF library. */
export function PrintSheet() {
  return (
    <Button variant="outline" size="sm" onClick={() => window.print()} data-print="hide">
      <Printer aria-hidden />
      Print
    </Button>
  );
}
