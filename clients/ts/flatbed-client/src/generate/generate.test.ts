import assert from "node:assert/strict";
import { test } from "node:test";

import { fetchInput } from "./generate.js";

test("fetchInput fetches openapi.json and schema.bfbs through the given fetch", () => {
  const calls: string[] = [];
  const mockFetch = ((url: string) => {
    calls.push(String(url));
    return Promise.resolve(
      String(url).endsWith("/openapi.json")
        ? new Response(JSON.stringify({ paths: {} }), { status: 200 })
        : new Response(new Uint8Array([1, 2, 3]), { status: 200 }),
    );
  }) as unknown as typeof fetch;
  // Trailing slash on the base is trimmed before the path is appended.
  return fetchInput("http://svc/", mockFetch).then((input) => {
    assert.deepEqual([...calls].sort(), ["http://svc/openapi.json", "http://svc/schema.bfbs"]);
    assert.deepEqual(input.spec, { paths: {} });
    assert.deepEqual(input.bfbs, new Uint8Array([1, 2, 3]));
  });
});

test("fetchInput rejects with the status when a response is not ok", () => {
  const mockFetch = (() =>
    Promise.resolve(new Response("nope", { status: 503, statusText: "Service Unavailable" }))) as unknown as typeof fetch;
  return assert.rejects(() => fetchInput("http://svc", mockFetch), /schema\.bfbs failed: 503|openapi\.json failed: 503/);
});
