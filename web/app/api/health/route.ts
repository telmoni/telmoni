import { NextResponse } from "next/server";

export async function GET() {
  // An intermediary that cached this would keep answering "ok" for a process
  // that is not.
  return NextResponse.json(
    { status: "ok" },
    { headers: { "cache-control": "no-store" } },
  );
}
