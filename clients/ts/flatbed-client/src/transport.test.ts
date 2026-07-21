import assert from "node:assert/strict";
import { test } from "node:test";

import { fetchTransport } from "./transport.js";

/** A minimal `fetch` stand-in that records its call and returns bytes. */
function stubFetch(status: number, body: Uint8Array) {
  const calls: { url: string; init?: RequestInit }[] = [];
  const impl = (async (url: string, init?: RequestInit) => {
    calls.push({ url, init });
    return {
      status,
      ok: status >= 200 && status < 300,
      arrayBuffer: async () => body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength),
    };
  }) as unknown as typeof globalThis.fetch;
  return { impl, calls };
}

test("fetchTransport forwards method, url, headers, and body", async () => {
  const { impl, calls } = stubFetch(200, new Uint8Array());
  const t = fetchTransport(impl);
  await t.send({
    method: "POST",
    url: "http://svc/echo",
    headers: { accept: "application/x-flatbuffers", "content-type": "application/x-flatbuffers" },
    body: new Uint8Array([1, 2]),
  });
  assert.equal(calls[0].url, "http://svc/echo");
  assert.equal(calls[0].init?.method, "POST");
  assert.deepEqual(calls[0].init?.body, new Uint8Array([1, 2]));
});

test("fetchTransport omits the body when none is given", async () => {
  const { impl, calls } = stubFetch(200, new Uint8Array());
  const t = fetchTransport(impl);
  await t.send({ method: "GET", url: "http://svc/h", headers: {} });
  assert.equal(calls[0].init?.body, undefined);
});

test("fetchTransport maps status, ok, and the response bytes", async () => {
  const { impl } = stubFetch(503, new Uint8Array([4, 5, 6]));
  const t = fetchTransport(impl);
  const res = await t.send({ method: "GET", url: "http://svc/h", headers: {} });
  assert.equal(res.status, 503);
  assert.equal(res.ok, false);
  assert.deepEqual(res.body, new Uint8Array([4, 5, 6]));
});
