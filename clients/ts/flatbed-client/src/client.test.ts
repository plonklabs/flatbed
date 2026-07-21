import assert from "node:assert/strict";
import { test } from "node:test";

import { FlatbedClient, type ClientOptions } from "./client.js";
import { FlatbedError } from "./error.js";
import type { FlatbedRequest, FlatbedResponse, Transport } from "./transport.js";

/** Captures the outgoing requests and returns a canned reply. */
class Recorder implements Transport {
  readonly requests: FlatbedRequest[] = [];
  constructor(private readonly reply: FlatbedResponse) {}
  async send(req: FlatbedRequest): Promise<FlatbedResponse> {
    this.requests.push(req);
    return this.reply;
  }
}

/** Exposes the protected `request` the way a generated client would call it. */
class TestClient extends FlatbedClient {
  call(method: string, path: string, body: Uint8Array): Promise<Uint8Array> {
    return this.request(method, path, body, (bytes) => bytes);
  }
}

function ok(body: Uint8Array = new Uint8Array()): FlatbedResponse {
  return { status: 200, ok: true, body };
}

function client(reply: FlatbedResponse, opts?: Partial<ClientOptions>): [TestClient, Recorder] {
  const transport = new Recorder(reply);
  return [new TestClient({ baseUrl: "http://svc", transport, ...opts }), transport];
}

test("POST sends the Content-Type and the body", async () => {
  const [c, t] = client(ok());
  await c.call("POST", "/echo", new Uint8Array([1, 2, 3]));
  assert.equal(t.requests[0].headers["content-type"], "application/x-flatbuffers");
  assert.deepEqual(t.requests[0].body, new Uint8Array([1, 2, 3]));
});

test("POST with an empty body still sends a Content-Type", async () => {
  // The server 415s a POST/PUT/PATCH that arrives without one.
  const [c, t] = client(ok());
  await c.call("POST", "/refresh", new Uint8Array());
  assert.equal(t.requests[0].headers["content-type"], "application/x-flatbuffers");
  assert.deepEqual(t.requests[0].body, new Uint8Array());
});

test("GET attaches neither a body nor a Content-Type", async () => {
  const [c, t] = client(ok());
  await c.call("GET", "/health", new Uint8Array([9]));
  assert.equal(t.requests[0].headers["content-type"], undefined);
  assert.equal(t.requests[0].body, undefined);
});

test("HEAD attaches neither a body nor a Content-Type", async () => {
  const [c, t] = client(ok());
  await c.call("HEAD", "/health", new Uint8Array([9]));
  assert.equal(t.requests[0].headers["content-type"], undefined);
  assert.equal(t.requests[0].body, undefined);
});

test("the Accept header is always application/x-flatbuffers", async () => {
  const [c, t] = client(ok());
  await c.call("GET", "/health", new Uint8Array());
  assert.equal(t.requests[0].headers.accept, "application/x-flatbuffers");
});

test("trailing slashes in baseUrl are trimmed before joining", async () => {
  const [c, t] = client(ok(), { baseUrl: "http://svc/" });
  await c.call("GET", "/health", new Uint8Array());
  assert.equal(t.requests[0].url, "http://svc/health");
});

test("a non-2xx response throws FlatbedError carrying the status", async () => {
  const [c] = client({ status: 404, ok: false, body: new Uint8Array() });
  await assert.rejects(
    () => c.call("GET", "/missing", new Uint8Array()),
    (e: unknown) => e instanceof FlatbedError && e.status === 404,
  );
});

test("decode receives the response bytes", async () => {
  const [c] = client(ok(new Uint8Array([7, 8])));
  const out = await c.call("GET", "/x", new Uint8Array());
  assert.deepEqual(out, new Uint8Array([7, 8]));
});

test("a custom transport replaces fetch without touching client code", async () => {
  const [c, t] = client(ok(new Uint8Array([1])));
  const out = await c.call("POST", "/x", new Uint8Array([2]));
  assert.equal(t.requests.length, 1);
  assert.deepEqual(out, new Uint8Array([1]));
});
