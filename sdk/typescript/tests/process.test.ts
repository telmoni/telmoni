/**
 * The SDK in a process of its own, as a script or a hook runs it: nothing
 * but the SDK holds the process, which the suite's own worker would.
 */

import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

const BUILT = fileURLToPath(new URL('../dist/index.mjs', import.meta.url));

function runScript(body: string): { status: number | null; stdout: string } {
  // Counted as the process exits, so a drain after the script's end is seen.
  const script = `
    import { writeSync } from 'node:fs';
    import { Telmoni } from ${JSON.stringify(BUILT)};
    let exports = 0;
    process.on('exit', () => writeSync(1, JSON.stringify({ exports })));
    const telmoni = new Telmoni({ apiKey: 'telmoni_test_key' }, {
      transport: async () => {
        exports += 1;
        return { status: 200, headers: { get: () => 'off' }, text: async () => '{}' };
      },
    });
    ${body}
  `;
  const result = spawnSync(process.execPath, ['--input-type=module', '-e', script], { encoding: 'utf8', timeout: 20_000 });
  return { status: result.status, stdout: result.stdout };
}

describe('a short-lived process', () => {
  it('sends what it flushes before it exits', () => {
    expect(existsSync(BUILT), 'build the SDK first: npm run build').toBe(true);
    const { status, stdout } = runScript(`
      telmoni.run('bot', () => undefined);
      if (!(await telmoni.flush())) process.exit(2);
    `);
    expect(status).toBe(0);
    expect(JSON.parse(stdout)).toEqual({ exports: 1 });
  });

  it('sends a record it flushes before it exits', () => {
    const { status, stdout } = runScript(`
      telmoni.recordRun({ runId: '3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d', agent: 'bot', start: 1000, end: 2000 });
      if (!(await telmoni.flush())) process.exit(2);
    `);
    expect(status).toBe(0);
    expect(JSON.parse(stdout)).toEqual({ exports: 1 });
  });

  it('sends what it queued when it ends of itself, neither flushed nor shut down', () => {
    const { status, stdout } = runScript(`
      telmoni.run('bot', () => undefined);
    `);
    expect(status).toBe(0);
    expect(JSON.parse(stdout)).toEqual({ exports: 1 });
  });
});
