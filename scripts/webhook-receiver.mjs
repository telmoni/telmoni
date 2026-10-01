// A laptop's stand-in for a customer's endpoint, behind a Cloudflare tunnel.
// Dev tooling only — nothing deploys it.
//
//   make webhook-receiver SECRET=whsec_...   behind a named route (:9000)
//   make webhook-tunnel                      quick tunnel, receiver and all
//
// Verifies `Telmoni-Signature` exactly as the docs site's Webhooks page tells a
// customer to (the same steps as `web/lib/webhook-signature.ts`, kept
// dependency-free so it runs with nothing installed), prints what arrived, and
// answers 204 on a good signature or 401 on a bad one. The 401 is deliberate:
// the first notice fires the moment an endpoint is connected, usually before
// the secret has been pasted here, and a non-2xx makes the loop retry it thirty
// seconds later with a fresh signature instead of marking it delivered
// unverified.
import { createHmac, timingSafeEqual } from "node:crypto";
import { createServer } from "node:http";

const PORT = Number(process.env.PORT ?? 9000);
const SECRET = process.env.TELMONI_WEBHOOK_SECRET ?? "";
const REPLAY_TOLERANCE_SECS = 300;

// Any `v1` that matches is enough: during a rotation's overlap the header
// carries one per secret.
function verify(secret, header, rawBody) {
  let t;
  const candidates = [];
  for (const part of header.split(",")) {
    const [key, value] = part.split("=", 2);
    if (key === "t") t = Number(value);
    else if (key === "v1") candidates.push(value);
  }
  if (!Number.isInteger(t) || candidates.length === 0) {
    return { ok: false, why: "malformed header" };
  }
  const skew = Math.abs(Math.floor(Date.now() / 1000) - t);
  if (skew > REPLAY_TOLERANCE_SECS) return { ok: false, why: `t is ${skew}s from now` };
  const expected = createHmac("sha256", secret).update(`${t}.`).update(rawBody).digest();
  const matched = candidates.some((v1) => {
    const given = Buffer.from(v1, "hex");
    return given.length === expected.length && timingSafeEqual(given, expected);
  });
  return matched ? { ok: true, why: "" } : { ok: false, why: "signature mismatch" };
}

// Delivery ids seen this run, so a retry after a 401 is labelled as the
// duplicate a real receiver would have to dedup.
const seen = new Set();

createServer((req, res) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const raw = Buffer.concat(chunks);
    const sig = req.headers["telmoni-signature"] ?? "";
    const id = req.headers["telmoni-delivery-id"] ?? "";
    const result = SECRET
      ? verify(SECRET, sig, raw)
      : { ok: false, why: "TELMONI_WEBHOOK_SECRET is not set" };
    const dup = seen.has(id);
    seen.add(id);
    console.log(
      `${new Date().toISOString()} ${req.method} ${req.url}\n` +
        `  delivery-id: ${id}${dup ? "  (DUPLICATE — a receiver dedups on this)" : ""}\n` +
        `  signature:   ${sig}\n` +
        `  verified:    ${result.ok}${result.ok ? "" : `  (${result.why})`}\n` +
        `  body:        ${raw.toString("utf8")}\n`,
    );
    res.writeHead(result.ok ? 204 : 401).end();
  });
}).listen(PORT, () => {
  console.log(
    `receiver listening on http://localhost:${PORT} — secret ${SECRET ? "set" : "NOT SET (every delivery will 401 until it is)"}`,
  );
});
