import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { inspect } from 'node:util';

import { SpanKind, SpanStatusCode } from '@opentelemetry/api';
import type { ReadableSpan } from '@opentelemetry/sdk-trace-base';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ANSWER_MAX, Exporter, fetchTransport } from '../src/exporter';
import { DEFAULT_ENDPOINT, Telmoni, VERSION, type Config } from '../src/index';
import { loadFixture } from './helpers';

afterEach(() => vi.unstubAllEnvs());

describe('configuration', () => {
  it('defaults to the machine host', () => {
    expect(DEFAULT_ENDPOINT).toBe('https://api.telmoni.com');
    const client = new Telmoni();
    expect(client.config.endpoint).toBe(DEFAULT_ENDPOINT);
    expect(client.config.captureContent).toBe(false);
    expect(client.config.sampleRate).toBe(1);
    expect(client.config.sampler).toBeUndefined();
  });

  it('strips trailing slashes', () => {
    const config: Config = {
      endpoint: 'https://api.telmoni.local///',
      apiKey: 'telmoni_test_key_123',
    };
    const client = new Telmoni(config);
    expect(client.config.endpoint).toBe('https://api.telmoni.local');
    expect(client.config.apiKey).toBe('telmoni_test_key_123');
    expect(new Telmoni({ endpoint: 'https://api.telmoni.local/' }).config.endpoint).toBe('https://api.telmoni.local');
  });

  it('reads a blank endpoint, organization, key, environment or release as unset', () => {
    const client = new Telmoni({ endpoint: '  ', organizationId: ' ', apiKey: '\t', environment: ' ', release: '' });
    expect(client.config.endpoint).toBe(DEFAULT_ENDPOINT);
    expect(client.config.organizationId).toBeUndefined();
    expect(client.config.apiKey).toBeUndefined();
    expect(client.config.environment).toBeUndefined();
    expect(client.config.release).toBeUndefined();
    const deployed = new Telmoni({ environment: ' staging ', release: '2026.10.18' });
    expect(deployed.config.environment).toBe('staging');
    expect(deployed.config.release).toBe('2026.10.18');
  });

  it('never prints or serializes the API key', () => {
    const client = new Telmoni({ apiKey: 'telmoni_secret_key', organizationId: 'org_1' });
    for (const printed of [
      inspect(client),
      JSON.stringify(client),
      inspect(client.config),
      JSON.stringify(client.config),
    ]) {
      expect(printed).not.toContain('telmoni_secret_key');
      expect(printed).toContain('org_1');
    }
    expect(client.config.apiKey).toBe('telmoni_secret_key');
  });

  it('reads the five variables from the environment and no other', () => {
    vi.stubEnv('TELMONI_ENDPOINT', undefined);
    vi.stubEnv('TELMONI_API_KEY', '');
    vi.stubEnv('TELMONI_ORG', 'org_2');
    vi.stubEnv('TELMONI_ENVIRONMENT', 'staging');
    vi.stubEnv('TELMONI_RELEASE', '');
    vi.stubEnv('OTEL_EXPORTER_OTLP_ENDPOINT', 'https://elsewhere.example');
    vi.stubEnv('OTEL_RESOURCE_ATTRIBUTES', 'deployment.environment.name=leaked,service.version=leaked');
    const client = Telmoni.fromEnv();
    expect(client.config.endpoint).toBe(DEFAULT_ENDPOINT);
    expect(client.config.apiKey).toBeUndefined();
    expect(client.config.organizationId).toBe('org_2');
    expect(client.config.environment).toBe('staging');
    expect(client.config.release).toBeUndefined();
  });

  it('takes the endpoint, key and release from the environment when set', () => {
    vi.stubEnv('TELMONI_ENDPOINT', 'https://api.telmoni.local/');
    vi.stubEnv('TELMONI_API_KEY', 'telmoni_env_key');
    vi.stubEnv('TELMONI_ORG', '');
    vi.stubEnv('TELMONI_ENVIRONMENT', undefined);
    vi.stubEnv('TELMONI_RELEASE', ' 2026.10.18 ');
    const client = Telmoni.fromEnv();
    expect(client.config.endpoint).toBe('https://api.telmoni.local');
    expect(client.config.apiKey).toBe('telmoni_env_key');
    expect(client.config.organizationId).toBeUndefined();
    expect(client.config.environment).toBeUndefined();
    expect(client.config.release).toBe('2026.10.18');
  });

  it('holds the sample rate to a number above zero and up to one', () => {
    expect(new Telmoni({ sampleRate: 1 }).config.sampleRate).toBe(1);
    expect(new Telmoni({ sampleRate: 0.25 }).config.sampleRate).toBe(0.25);
    expect(() => new Telmoni({ sampleRate: 0 })).toThrow(RangeError);
    expect(() => new Telmoni({ sampleRate: 1.5 })).toThrow(RangeError);
    expect(() => new Telmoni({ sampleRate: Number.NaN })).toThrow(RangeError);
    for (const notANumber of ['0.5', true, 1n]) {
      expect(() => new Telmoni({ sampleRate: notANumber as unknown as number })).toThrow(RangeError);
    }
    const config = new Telmoni({ sampleRate: 0.25 }).config as { sampleRate: unknown };
    expect(() => {
      config.sampleRate = '0.5';
    }).toThrow(TypeError);
  });

  it('is the version package.json names', () => {
    const pkg = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8')) as { version: string };
    expect(VERSION).toBe(pkg.version);
  });

  it('builds from the environment with the real transport', async () => {
    // The shell running the suite may point the CLI at a local stack; the
    // SDK's default is what is under test.
    vi.stubEnv('TELMONI_ENDPOINT', undefined);
    const client = Telmoni.fromEnv();
    try {
      expect(client.config.endpoint).toBe(DEFAULT_ENDPOINT);
    } finally {
      await client.shutdown();
    }
  });
});

describe('the endpoint', () => {
  it('is read as the fixture reads it: plain HTTP to this machine alone, and a host without a scheme as the CLI reads it', () => {
    const fixture = loadFixture<{
      taken: { given: string; endpoint: string }[];
      refused: { given: string; as: 'url' | 'cleartext' }[];
    }>('endpoints.json');
    for (const { given, endpoint } of fixture.taken) {
      expect(new Telmoni({ endpoint: given }).config.endpoint, given).toBe(endpoint);
      expect(new Telmoni({ endpoint }).config.endpoint, `${given}, read twice`).toBe(endpoint);
    }
    for (const { given, as } of fixture.refused) {
      const why = as === 'cleartext' ? /unencrypted HTTP/ : /the endpoint must be an http:\/\/ or https:\/\/ URL/;
      expect(() => new Telmoni({ endpoint: given }), given).toThrow(why);
    }
  });
});

describe('the transport', () => {
  it('reads an answer up to its first mebibyte', async () => {
    // Each path's status and the length of its answer.
    const answers: Record<string, [number, number]> = {
      '/kept': [200, 2 << 20],
      '/refused': [503, 2 << 20],
      '/short': [200, 300],
    };
    const server = createServer((request, response) => {
      request.resume().on('end', () => {
        const [status, size] = answers[request.url ?? ''] ?? [404, 0];
        response.writeHead(status, { 'Content-Length': size }).end('x'.repeat(size));
      });
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const { port } = server.address() as AddressInfo;
    try {
      for (const [path, status, read] of [
        ['/kept', 200, ANSWER_MAX],
        ['/refused', 503, ANSWER_MAX],
        ['/short', 200, 300],
      ] as const) {
        const answer = await fetchTransport(`http://127.0.0.1:${port}${path}`, { method: 'POST', headers: {}, body: '{}' });
        expect([answer.status, (await answer.text()).length]).toEqual([status, read]);
      }
    } finally {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  });

  // A refusal's problem, sent whole before its answer hangs up short of the length it declared.
  const PROBLEM = '{"message":"/errors/batch-too-large: the batch is too large"}';
  // Each body the server was sent.
  const breaking = async (status: number) => {
    const bodies: string[] = [];
    const server = createServer((request, response) => {
      let body = '';
      request.setEncoding('utf8');
      request.on('data', (chunk: string) => (body += chunk));
      request.on('end', () => {
        bodies.push(body);
        response.writeHead(status, { 'Content-Length': 300 });
        response.write(PROBLEM, () => response.socket?.end());
      });
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const close = async () => {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    };
    return { bodies, close, url: `http://127.0.0.1:${(server.address() as AddressInfo).port}` };
  };

  it('keeps an answer whose body breaks as far as it came, its status deciding', async () => {
    for (const status of [200, 413]) {
      const server = await breaking(status);
      try {
        const answer = await fetchTransport(server.url, { method: 'POST', headers: {}, body: '{}' });
        expect([answer.status, await answer.text()]).toEqual([status, PROBLEM]);
      } finally {
        await server.close();
      }
    }
  });

  it('sends a refusal whose body breaks once, as any refusal', async () => {
    const server = await breaking(413);
    const sleeps: number[] = [];
    const warnings: string[] = [];
    try {
      const telmoni = new Telmoni(
        { endpoint: server.url, apiKey: 'k' },
        {
          sleep: (seconds) => {
            sleeps.push(seconds);
            return Promise.resolve();
          },
          warn: (message) => warnings.push(message),
        },
      );
      telmoni.run('bot', () => undefined);
      await telmoni.shutdown();
    } finally {
      await server.close();
    }
    expect(server.bodies.length).toBeGreaterThan(0);
    expect(new Set(server.bodies).size).toBe(server.bodies.length);
    expect(sleeps).toEqual([]);
    expect(warnings).toEqual(['telmoni: export refused with 413 /errors/batch-too-large; the batch was dropped']);
  });
});

describe('a span time', () => {
  it('goes as 0 outside 64 bits of nanoseconds, so ingest refuses that span rather than the whole export', () => {
    const config = new Telmoni({ apiKey: 'k' }).config;
    const exporter = new Exporter(config, {
      transport: () => Promise.reject(new Error('never sent')),
      sleep: () => Promise.resolve(),
      warn: () => {},
    });
    const span = (startTime: [number, number]): ReadableSpan =>
      ({
        name: 's',
        kind: SpanKind.INTERNAL,
        spanContext: () => ({ traceId: `${'0'.repeat(31)}1`, spanId: `${'0'.repeat(15)}1`, traceFlags: 1 }),
        parentSpanContext: undefined,
        startTime,
        endTime: startTime,
        attributes: {},
        status: { code: SpanStatusCode.UNSET },
      }) as unknown as ReadableSpan;
    const encoded = exporter.encode([span([18_446_744_073, 709_551_615]), span([18_446_744_073, 709_551_616]), span([-1, 0])]);
    const sent = encoded.resourceSpans[0]!.scopeSpans[0]!.spans.map((each) => each.startTimeUnixNano);
    expect(sent).toEqual(['18446744073709551615', '0', '0']);
  });
});
