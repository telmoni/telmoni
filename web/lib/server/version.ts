// The release this console was built as. Its package.json carries the
// server's version, the workspace's in Cargo.toml, since the two ship
// together under one tag. Read here, on the server, and handed to the rail as
// a prop, the number reaches only the pages a session renders: in a script,
// anyone could fetch it and learn which release's advisories to try.
import "server-only";

import packageJson from "@/package.json";

export const VERSION: string = packageJson.version;
