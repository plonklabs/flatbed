import assert from "node:assert/strict";
import { test } from "node:test";

import { request, type ClientConfig } from "./client.js";
import { FlatbedError } from "./error.js";
import type { FlatbedRequest, FlatbedResponse, Transport } from "./transport.js";

/** Captures the outgoing requests and returns a canned reply. */
class Recorder implements Transport {
  readonly requests: FlatbedRequest[] = [];
  constructor(private readonly reply: FlatbedResponse) {}
  send(req: FlatbedRequest): Promise<FlatbedResponse> {
    this.requests.push(req);
    return Promise.resolve(this.reply);
  }
}

const ok = (body: Uint8Array = new Uint8Array()): FlatbedResponse => ({ status: 200, ok: true, body });

const identity = (bytes: Uint8Array): Uint8Array => bytes;

const call = (
  reply: FlatbedResponse,
  method: string,
  path: string,
  body: Uint8Array,
  extra?: Partial<ClientConfig>,
): Promise<{ out: Uint8Array; sent: FlatbedRequest }> => {
  const transport = new Recorder(reply);
  return request({ baseUrl: "http://svc", transport, ...extra }, method, path, body, identity).then((out) => ({
    out,
    sent: transport.requests[0]!,
  }));
};

test("POST sends the Content-Type and the body", () =>
  call(ok(), "POST", "/echo", new Uint8Array([1, 2, 3])).then(({ sent }) => {
    assert.equal(sent.headers["content-type"], "application/x-flatbuffers");
    assert.deepEqual(sent.body, new Uint8Array([1, 2, 3]));
  }));

test("POST with an empty body still sends a Content-Type", () =>
  // The server 415s a POST/PUT/PATCH that arrives without one.
  call(ok(), "POST", "/refresh", new Uint8Array()).then(({ sent }) => {
    assert.equal(sent.headers["content-type"], "application/x-flatbuffers");
    assert.deepEqual(sent.body, new Uint8Array());
  }));

test("GET attaches neither a body nor a Content-Type", () =>
  call(ok(), "GET", "/health", new Uint8Array([9])).then(({ sent }) => {
    assert.equal(sent.headers["content-type"], undefined);
    assert.equal(sent.body, undefined);
  }));

test("HEAD attaches neither a body nor a Content-Type", () =>
  call(ok(), "HEAD", "/health", new Uint8Array([9])).then(({ sent }) => {
    assert.equal(sent.headers["content-type"], undefined);
    assert.equal(sent.body, undefined);
  }));

test("the Accept header is always application/x-flatbuffers", () =>
  call(ok(), "GET", "/health", new Uint8Array()).then(({ sent }) => {
    assert.equal(sent.headers.accept, "application/x-flatbuffers");
  }));

test("config headers are added to every request", () =>
  call(ok(), "GET", "/health", new Uint8Array(), { headers: { authorization: "Bearer t" } }).then(({ sent }) => {
    assert.equal(sent.headers.authorization, "Bearer t");
    assert.equal(sent.headers.accept, "application/x-flatbuffers");
  }));

test("trailing slashes in baseUrl are trimmed before joining", () =>
  call(ok(), "GET", "/health", new Uint8Array(), { baseUrl: "http://svc/" }).then(({ sent }) => {
    assert.equal(sent.url, "http://svc/health");
  }));

test("a non-2xx response rejects with FlatbedError carrying the status", () =>
  assert.rejects(
    () => call({ status: 404, ok: false, body: new Uint8Array() }, "GET", "/missing", new Uint8Array()),
    (e: unknown) => e instanceof FlatbedError && e.status === 404,
  ));

test("decode receives the response bytes", () =>
  call(ok(new Uint8Array([7, 8])), "GET", "/x", new Uint8Array()).then(({ out }) => {
    assert.deepEqual(out, new Uint8Array([7, 8]));
  }));
