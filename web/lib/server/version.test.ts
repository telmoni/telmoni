import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import { VERSION } from "./version";

// A release bumps both manifests, and one bumped alone puts a number on the
// rail that the server does not run: the console's stood at the scaffold's
// 0.1.0 while the server's said 0.0.0.
describe("the console's version", () => {
  it("is the server's, the workspace's in Cargo.toml", () => {
    const cargo = readFileSync(
      fileURLToPath(new URL("../../../Cargo.toml", import.meta.url)),
      "utf8",
    );
    const workspace = cargo.split("[workspace.package]")[1]?.split(/\n\[/)[0] ?? "";
    expect(/^version\s*=\s*"([^"]+)"/m.exec(workspace)?.[1]).toBe(VERSION);
  });
});
