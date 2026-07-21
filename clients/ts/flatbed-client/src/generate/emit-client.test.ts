import assert from "node:assert/strict";
import { test } from "node:test";

import { checkNames, emitClient, emitIndex, methodName } from "./emit-client.js";

test("methodName camelCases an operationId", () => {
  assert.equal(methodName({ method: "POST", path: "/x", operationId: "CreateUser" }), "createUser");
});

test("methodName derives <method><PascalPath> without an operationId", () => {
  assert.equal(methodName({ method: "GET", path: "/users/{id}" }), "getUsersById");
});

test("emitClient extends the package base and binds the codec", () => {
  const out = emitClient([
    { method: "POST", path: "/echo", operationId: "echo", requestType: "EchoRequest", responseType: "EchoResponse" },
  ]);
  assert.match(out, /import \{ FlatbedClient as FlatbedClientBase \} from "@plonklabs\/flatbed-client";/);
  assert.match(out, /export class FlatbedClient extends FlatbedClientBase \{/);
  assert.match(out, /echo\(body: EchoRequest\): Promise<EchoResponse> \{/);
  assert.match(out, /codec\.encodeEchoRequestRoot\(body\)/);
  assert.match(out, /codec\.decodeEchoResponseRoot/);
});

test("a GET drops the body argument and encodes an empty payload", () => {
  const out = emitClient([{ method: "GET", path: "/health", responseType: "Health" }]);
  assert.match(out, /getHealth\(\): Promise<Health> \{/);
  assert.match(out, /new Uint8Array\(\)/);
});

test("path params become encoded leading string args", () => {
  const out = emitClient([{ method: "GET", path: "/users/{id}", responseType: "User" }]);
  assert.match(out, /getUsersById\(id: string\): Promise<User> \{/);
  assert.match(out, /encodeURIComponent\(id\)/);
});

test("no codec/type imports when no operation is typed", () => {
  const out = emitClient([{ method: "GET", path: "/ping" }]);
  assert.doesNotMatch(out, /import \* as codec/);
  assert.doesNotMatch(out, /import type/);
  assert.match(out, /getPing\(\): Promise<Uint8Array> \{/);
});

test("checkNames rejects duplicate derived method names", () => {
  assert.throws(
    () =>
      checkNames([
        { method: "GET", path: "/x", operationId: "dup" },
        { method: "POST", path: "/y", operationId: "dup", requestType: "R" },
      ]),
    /same client method `dup`/,
  );
});

test("checkNames rejects a name reserved by the base client", () => {
  assert.throws(
    () => checkNames([{ method: "POST", path: "/x", operationId: "request", requestType: "R" }]),
    /collides/,
  );
});

test("checkNames rejects a path param colliding with the body argument", () => {
  assert.throws(
    () => checkNames([{ method: "POST", path: "/x/{body}", requestType: "R" }]),
    /request-body argument/,
  );
});

test("emitIndex re-exports the whole folder", () => {
  const out = emitIndex();
  assert.match(out, /export \* from ".\/types.js";/);
  assert.match(out, /export \* from ".\/client.js";/);
  assert.match(out, /export \* as codec from ".\/codec.js";/);
});
