/**
 * A request body, read with a ceiling that holds for a body that never
 * declares its length.
 *
 * ⚠ **`content-length` is a claim, not a limit.** A sender that omits it, or
 * chunks the body, or simply lies, gets past a check on the header alone —
 * and `request.arrayBuffer()` then buffers everything it sends before any
 * code can look at the size. The two webhook relays are public, unauthenticated
 * doors that must forward the raw bytes for a signature check downstream, so
 * they were exactly the place to be handed a body the size of the process's
 * heap. This reads the stream and stops the moment the ceiling is crossed.
 *
 * `null` means "over the ceiling": the caller answers 413 and never sees the
 * bytes. The declared length is still checked first, so an honest oversize
 * request costs nothing to refuse.
 */
export async function readBodyCapped(
  request: Request,
  maxBytes: number,
): Promise<Uint8Array<ArrayBuffer> | null> {
  const declared = request.headers.get("content-length");
  if (declared !== null && Number(declared) > maxBytes) return null;
  if (request.body === null) return new Uint8Array(0);

  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let received = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      received += value.byteLength;
      if (received > maxBytes) {
        await reader.cancel();
        return null;
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }

  const body = new Uint8Array(received);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}
