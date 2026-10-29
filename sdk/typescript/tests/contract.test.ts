import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { existsSync, readFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import { createServer as createNetServer, type AddressInfo } from 'node:net';
import { fileURLToPath } from 'node:url';
import { inspect } from 'node:util';

import { SpanKind, SpanStatusCode } from '@opentelemetry/api';
import type { ReadableSpan } from '@opentelemetry/sdk-trace-base';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { ANSWER_MAX, Exporter, directTransport, fetchTransport } from '../src/exporter';
import { DEFAULT_ENDPOINT, Telmoni, VERSION, type Config } from '../src/index';
import { FakeTransport, answer, harness, loadFixture } from './helpers';

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

  it("leaves off a deployment named past ingest's cap, with one warning", async () => {
    // Ingest would read the resource as naming none, without a word.
    const h = harness({ environment: 'e'.repeat(129), release: 'r'.repeat(128) });
    h.telmoni.run('bot', () => {});
    expect(await h.telmoni.flush()).toBe(true);
    const keys = h.transport.bodies()[0]!.resourceSpans[0]!.resource.attributes.map((attribute) => attribute.key);
    expect(keys).not.toContain('deployment.environment.name');
    expect(keys).toContain('service.version');
    expect(h.warnings).toEqual(["telmoni: the environment must be up to 128 characters; it is not sent"]);
  });

  it("reads blank as Unicode's White_Space, as Go and Rust read it", () => {
    // `trim` would take U+FEFF too, and leave U+0085.
    expect(new Telmoni({ apiKey: 'k', release: '\ufeff2026.10.18' }).config.release).toBe('\ufeff2026.10.18');
    expect(new Telmoni({ apiKey: 'k', release: '\u00852026.10.18\u3000' }).config.release).toBe('2026.10.18');
    expect(new Telmoni({ apiKey: 'k', environment: '\u0085' }).config.environment).toBeUndefined();
    const h = harness();
    expect(() => h.telmoni.run('\u0085', () => {})).toThrow("a run needs its agent's name");
    expect(() => h.telmoni.run('\ufeff', () => {})).not.toThrow();
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

// A refusal's problem, sent whole before its answer hangs up short of the length it declared.
const PROBLEM = '{"message":"/errors/batch-too-large: the batch is too large"}';

// A server answering `status` with PROBLEM and hanging up short; each body it was sent.
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

// A server answering each request, read whole, with `reply` as it stands, then the close.
const raw = async (reply: string) => {
  const bodies: string[] = [];
  const server = createNetServer((socket) => {
    let seen = Buffer.alloc(0);
    socket.on('error', () => {});
    socket.on('data', (chunk: Buffer) => {
      seen = Buffer.concat([seen, chunk]);
      const end = seen.indexOf('\r\n\r\n');
      if (end < 0) return;
      const length = Number(/content-length:\s*(\d+)/i.exec(seen.subarray(0, end).toString())?.[1] ?? 0);
      if (seen.length < end + 4 + length) return;
      bodies.push(seen.subarray(end + 4, end + 4 + length).toString());
      socket.end(reply);
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const close = () => new Promise((resolve) => server.close(resolve));
  return { bodies, close, url: `http://127.0.0.1:${(server.address() as AddressInfo).port}` };
};

describe.each([
  ['fetch', fetchTransport],
  ['direct', directTransport],
] as const)('the %s transport', (_name, transport) => {
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
        const answer = await transport(`http://127.0.0.1:${port}${path}`, { method: 'POST', headers: {}, body: '{}' });
        expect([answer.status, (await answer.text()).length]).toEqual([status, read]);
      }
    } finally {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  });

  it('keeps an answer whose body breaks as far as it came, its status deciding', async () => {
    for (const status of [200, 413]) {
      const server = await breaking(status);
      try {
        const answer = await transport(server.url, { method: 'POST', headers: {}, body: '{}' });
        expect([answer.status, await answer.text()]).toEqual([status, PROBLEM]);
      } finally {
        await server.close();
      }
    }
  });

  it('takes a head cut before its blank line for no answer', async () => {
    const server = await raw('HTTP/1.1 413 Payload Too Large\r\nContent-Ty');
    try {
      await expect(transport(server.url, { method: 'POST', headers: {}, body: '{}' })).rejects.toThrow();
    } finally {
      await server.close();
    }
  });

  it('reads the answer past every 1xx before it', async () => {
    const server = await raw(
      'HTTP/1.1 103 Early Hints\r\nLink: </x>\r\n\r\nHTTP/1.1 102 Processing\r\n\r\n' +
        'HTTP/1.1 200 OK\r\nTelmoni-Content-Mode: on\r\nContent-Length: 2\r\n\r\n{}',
    );
    try {
      const answer = await transport(server.url, { method: 'POST', headers: {}, body: '{}' });
      expect([answer.status, answer.headers.get('Telmoni-Content-Mode'), await answer.text()]).toEqual([200, 'on', '{}']);
    } finally {
      await server.close();
    }
  });

  it('takes an upgrade nobody asked for as no answer, rather than wait on it', async () => {
    const server = await raw('HTTP/1.1 101 Switching Protocols\r\nUpgrade: x\r\nConnection: Upgrade\r\n\r\n');
    try {
      await expect(transport(server.url, { method: 'POST', headers: {}, body: '{}' })).rejects.toThrow();
    } finally {
      await server.close();
    }
  });
});

describe('the exporter, sending to this machine', () => {
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

  it('sends an answer whose head is cut three times, as no answer', async () => {
    const server = await raw('HTTP/1.1 413 Payload Too Large\r\nContent-Ty');
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
    const tries = new Map<string, number>();
    for (const body of server.bodies) tries.set(body, (tries.get(body) ?? 0) + 1);
    expect(tries.size).toBeGreaterThan(0);
    expect(new Set(tries.values())).toEqual(new Set([3]));
    expect(sleeps).toEqual(Array.from({ length: tries.size }, () => [1, 2]).flat());
    expect(warnings).toHaveLength(1);
    expect(warnings[0]).toMatch(/^telmoni: export failed after 3 attempts \(\w+\); the batch was dropped$/);
  });

  it('reaches this machine past any proxy the environment or the application names', async () => {
    const built = fileURLToPath(new URL('../dist/index.mjs', import.meta.url));
    expect(existsSync(built), 'build the SDK first: npm run build').toBe(true);
    // The host the proxy was asked for, by a request or a tunnel, and the keys the
    // server itself was sent for the export.
    const taken: string[] = [];
    const keys: string[] = [];
    const proxy = createServer((request, response) => {
      taken.push(new URL(request.url ?? '', 'http://unasked').host);
      request.resume().on('end', () => response.end('{}'));
    });
    // Node 22's fetch tunnels to the environment's proxy with CONNECT, and dials again
    // for as long as one goes unanswered: refused here, it fails at once.
    proxy.on('connect', (request, socket) => {
      taken.push(request.url ?? '');
      socket.end('HTTP/1.1 502 Bad Gateway\r\nConnection: close\r\n\r\n');
    });
    const server = createServer((request, response) => {
      if (request.url === '/otlp/v1/traces') keys.push(request.headers.authorization ?? '');
      request.resume().on('end', () => response.end('{}'));
    });
    const port = async (listening: Server) => {
      await new Promise<void>((resolve) => listening.listen(0, '127.0.0.1', resolve));
      return (listening.address() as AddressInfo).port;
    };
    const named = `http://127.0.0.1:${await port(proxy)}`;
    const at = `http://127.0.0.1:${await port(server)}`;
    try {
      // An application's own proxy, as proxy-agent and global-agent set one: Node's
      // global agent replaced, and \`http.request\` wrapped to force it whatever is asked.
      const script = `
        import http from 'node:http';
        import { syncBuiltinESMExports } from 'node:module';
        class Proxying extends http.Agent {
          addRequest(request, options) {
            request.path = 'http://' + options.host + ':' + options.port + request.path;
            return super.addRequest(request, { ...options, host: '127.0.0.1', port: ${new URL(named).port} });
          }
        }
        const agent = new Proxying();
        http.globalAgent = agent;
        const original = http.request;
        http.request = (url, options, callback) => {
          if (typeof options === 'function') [options, callback] = [{}, options];
          return original(url, { ...options, agent }, callback);
        };
        syncBuiltinESMExports();
        await fetch(${JSON.stringify(`${at}/fetch`)}, { method: 'POST', body: '{}' }).catch(() => {});
        await new Promise((resolve) => http.request(${JSON.stringify(`${at}/http`)}, { method: 'POST' }, (answer) => answer.resume().on('end', resolve)).on('error', resolve).end('{}'));
        const { Telmoni } = await import(${JSON.stringify(built)});
        const telmoni = new Telmoni({ endpoint: ${JSON.stringify(at)}, apiKey: 'k' });
        telmoni.run('bot', () => undefined);
        await telmoni.shutdown();
      `;
      const env = { ...process.env, NODE_USE_ENV_PROXY: '1', HTTP_PROXY: named, http_proxy: named, NO_PROXY: '', no_proxy: '' };
      const child = spawn(process.execPath, ['--input-type=module', '-e', script], { env, stdio: 'ignore' });
      await once(child, 'exit');
    } finally {
      for (const listening of [proxy, server]) {
        listening.closeAllConnections();
        await new Promise((resolve) => listening.close(resolve));
      }
    }
    // Node's own http went through the application's proxy, and its fetch through the
    // environment's where this Node honours the variable, as a tunnel or in the clear.
    const [major = 0, minor = 0] = process.versions.node.split('.').map(Number);
    const honoured = major >= 25 || (major === 24 && minor >= 5) || (major === 22 && minor >= 21);
    const host = new URL(at).host;
    expect(taken).toEqual(honoured ? [host, host] : [host]);
    expect(keys.length).toBeGreaterThan(0);
    expect(new Set(keys)).toEqual(new Set(['Bearer k']));
  });
});

describe("an export past ingest's 1 MiB or 1,000 spans", () => {
  // A span named `s<n>`, or `name`, for each size, an attribute of that many characters beside one of a few.
  const spans = (sizes: number[], char = 'x', name?: string): ReadableSpan[] =>
    sizes.map(
      (size, n) =>
        ({
          name: name ?? `s${n}`,
          kind: SpanKind.INTERNAL,
          spanContext: () => ({ traceId: `${'0'.repeat(31)}1`, spanId: `${'0'.repeat(15)}1`, traceFlags: 1 }),
          parentSpanContext: undefined,
          startTime: [1, 0],
          endTime: [2, 0],
          attributes: { 'telmoni.tag.big': char.repeat(size), 'app.kept': 'yes' },
          status: { code: SpanStatusCode.UNSET },
        }) as unknown as ReadableSpan,
    );
  const exporter = (transport: FakeTransport, warnings: string[]) =>
    new Exporter(new Telmoni({ apiKey: 'k' }).config, {
      transport: transport.send,
      sleep: () => Promise.resolve(),
      warn: (message) => warnings.push(message),
    });

  it('is halved until each part fits, its spans each sent once and in order', async () => {
    const transport = new FakeTransport();
    const warnings: string[] = [];
    await exporter(transport, warnings).export(spans(Array<number>(8).fill(300_000)));
    const sizes = transport.requests.map((request) => Buffer.byteLength(request.body));
    expect(sizes).toHaveLength(4);
    expect(Math.max(...sizes)).toBeLessThanOrEqual(1 << 20);
    expect(transport.spans().map((span) => span.name)).toEqual(Array.from({ length: 8 }, (_, n) => `s${n}`));
    expect(warnings).toEqual([]);
  });

  it('is counted in the bytes sent: two a character, past it in bytes and short of it in characters', async () => {
    const transport = new FakeTransport();
    await exporter(transport, []).export(spans(Array<number>(4).fill(200_000), 'é'));
    const sizes = transport.requests.map((request) => Buffer.byteLength(request.body));
    expect(sizes).toHaveLength(2);
    expect(Math.max(...sizes)).toBeLessThanOrEqual(1 << 20);
  });

  it('is halved past 1,000 spans until each part holds them, one of 1,000 going as it is', async () => {
    const transport = new FakeTransport();
    await exporter(transport, []).export(spans(Array<number>(2001).fill(1)));
    const counts = transport.bodies().map((body) => body.resourceSpans[0]!.scopeSpans[0]!.spans.length);
    expect(counts).toEqual([1000, 500, 501]);
    expect(transport.spans().map((span) => span.name)).toEqual(Array.from({ length: 2001 }, (_, n) => `s${n}`));
  });

  it('goes no further once a part fails as the rest would, and on past a part refused for what it holds', async () => {
    for (const [answers, sent, warned] of [
      [
        Array.from({ length: 9 }, () => new Error('refused')),
        3,
        'telmoni: export failed after 3 attempts (Error); the batch was dropped',
      ],
      [[answer(400, {}, '')], 3, 'telmoni: export refused with 400 ; the batch was dropped'],
    ] as const) {
      const transport = new FakeTransport([...answers]);
      const warnings: string[] = [];
      await exporter(transport, warnings).export(spans(Array<number>(2001).fill(1)));
      expect(transport.requests).toHaveLength(sent);
      expect(warnings).toEqual([warned]);
    }
  });

  it('measures a list as ingest writes it, its JSON, so a span past it for a list of short strings goes without the list', async () => {
    const transport = new FakeTransport();
    const [one] = spans([1]);
    (one!.attributes as Record<string, unknown>)['app.list'] = Array<string>(60_000).fill('');
    await exporter(transport, []).export([one!]);
    expect(transport.spans().map((span) => span.attributes.map((each) => each.key).sort())).toEqual([
      ['app.kept', 'telmoni.tag.big'],
    ]);
  });

  it('sends a span alone past it without what ingest drops for its length, rather than lose it', async () => {
    const transport = new FakeTransport();
    const warnings: string[] = [];
    const sender = exporter(transport, warnings);
    await sender.export(spans([100, 1 << 20, 100]));
    await sender.export(spans([1 << 20]));
    const kept = transport.spans().map((span) => [span.name, span.attributes.map((each) => each.key).sort()]);
    const both = ['app.kept', 'telmoni.tag.big'];
    expect(kept).toEqual([['s0', both], ['s1', ['app.kept']], ['s2', both], ['s0', ['app.kept']]]);
    expect(warnings).toEqual([
      'telmoni: a span past the 1 MiB an export may hold was sent without the attributes ingest drops for their length',
    ]);
  });

  it('drops a span whose own name passes it, with one warning', async () => {
    const transport = new FakeTransport();
    const warnings: string[] = [];
    await exporter(transport, warnings).export(spans([1], 'x', 'x'.repeat(1 << 20)));
    expect(transport.requests).toEqual([]);
    expect(warnings).toEqual(['telmoni: a span past the 1 MiB an export may hold was dropped']);
  });
});

describe('a number ingest cannot read', () => {
  it('goes as no value, NaN or an infinity as null and a count past 64 bits as a double, never costing its export', async () => {
    const transport = new FakeTransport();
    const exporter = new Exporter(new Telmoni({ apiKey: 'k' }).config, {
      transport: transport.send,
      sleep: () => Promise.resolve(),
      warn: () => {},
    });
    const span = (name: string, attributes: Record<string, unknown>): ReadableSpan =>
      ({
        name,
        kind: SpanKind.INTERNAL,
        spanContext: () => ({ traceId: `${'0'.repeat(31)}1`, spanId: `${'0'.repeat(15)}1`, traceFlags: 1 }),
        parentSpanContext: undefined,
        startTime: [1, 0],
        endTime: [2, 0],
        attributes,
        status: { code: SpanStatusCode.UNSET },
      }) as unknown as ReadableSpan;
    await exporter.export([
      span('odd', {
        'app.nan': Number.NaN,
        'app.below': -Infinity,
        'app.ratios': [1.5, Infinity],
        'app.ratio': 0.25,
        'gen_ai.usage.input_tokens': Number.NaN,
        'gen_ai.usage.cache_read.input_tokens': Infinity,
        'gen_ai.usage.output_tokens': 2 ** 64,
      }),
      span('beside', {}),
    ]);
    expect(transport.requests).toHaveLength(1);
    const [odd, beside] = transport.spans();
    expect(beside?.name).toBe('beside');
    expect(Object.fromEntries(odd!.attributes.map((each) => [each.key, each.value]))).toEqual({
      'app.nan': { doubleValue: null },
      'app.below': { doubleValue: null },
      'app.ratios': { arrayValue: { values: [{ doubleValue: 1.5 }, { doubleValue: null }] } },
      'app.ratio': { doubleValue: 0.25 },
      'gen_ai.usage.input_tokens': { doubleValue: null },
      'gen_ai.usage.cache_read.input_tokens': { doubleValue: null },
      'gen_ai.usage.output_tokens': { doubleValue: 2 ** 64 },
    });
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
