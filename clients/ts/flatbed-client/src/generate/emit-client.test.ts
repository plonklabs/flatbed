import assert from "node:assert/strict";
import { test } from "node:test";

import { checkNames, checkTypes, emitClient, emitIndex, methodName } from "./emit-client.js";
import type { FbsSchema } from "./model.js";

test("methodName camelCases an operationId", () => {
  assert.equal(methodName({ method: "POST", path: "/x", operationId: "CreateUser" }), "createUser");
});

test("methodName derives <method><PascalPath> without an operationId", () => {
  assert.equal(methodName({ method: "GET", path: "/users/{id}" }), "getUsersById");
});

test("emitClient generates a createFlatbedClient factory", () => {
  const out = emitClient([
    { method: "POST", path: "/echo", operationId: "echo", requestType: "EchoRequest", responseType: "EchoResponse" },
  ]);
  assert.match(out, /import \{ request, type ClientConfig \} from "@plonklabs\/flatbed-client";/);
  assert.match(out, /export const createFlatbedClient = \(config: ClientConfig\) => \(\{/);
  assert.match(out, /echo: \(args: \{ body: EchoRequest \}\): Promise<EchoResponse> =>/);
  assert.match(out, /request\(config, "POST", "\/echo", codec\.encodeEchoRequestRoot\(args\.body\), codec\.decodeEchoResponseRoot\)/);
});

test("a GET with a path param takes { pathParams } and no body", () => {
  const out = emitClient([{ method: "GET", path: "/users/{id}", responseType: "User" }]);
  assert.match(out, /getUsersById: \(args: \{ pathParams: \{ id: string \} \}\): Promise<User> =>/);
  assert.match(out, /encodeURIComponent\(args\.pathParams\.id\)/);
  assert.match(out, /new Uint8Array\(\)/);
});

test("a PUT with a body and a path param takes both keys", () => {
  const out = emitClient([{ method: "PUT", path: "/users/{id}", requestType: "UserPatch", responseType: "User" }]);
  assert.match(out, /putUsersById: \(args: \{ body: UserPatch; pathParams: \{ id: string \} \}\): Promise<User> =>/);
  assert.match(out, /codec\.encodeUserPatchRoot\(args\.body\)/);
});

test("an operation with neither body nor path params takes no argument", () => {
  const out = emitClient([{ method: "GET", path: "/health", responseType: "Health" }]);
  assert.match(out, /getHealth: \(\): Promise<Health> =>/);
});

test("no codec/type imports when no operation is typed", () => {
  const out = emitClient([{ method: "GET", path: "/ping" }]);
  assert.doesNotMatch(out, /import \* as codec/);
  assert.doesNotMatch(out, /import type/);
  assert.match(out, /getPing: \(\): Promise<Uint8Array> =>/);
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

test("checkNames rejects an operationId that derives an invalid identifier", () => {
  // "3D" camelCases to "3d" — a leading digit is not a valid TS identifier.
  assert.throws(
    () => checkNames([{ method: "GET", path: "/x", operationId: "3D" }]),
    /derives the client method name `3d`, which is not a valid TypeScript identifier/,
  );
});

test("checkNames rejects duplicate path-param keys in a path", () => {
  assert.throws(
    () => checkNames([{ method: "GET", path: "/users/{id}/posts/{id}" }]),
    /two path parameters that map to the key `id`/,
  );
});

const schema = (names: readonly string[]): FbsSchema => ({
  tables: names.map((name) => ({ name, fields: [] })),
  enums: [],
});

test("checkTypes rejects an operation referencing an unknown table", () => {
  assert.throws(
    () => checkTypes(schema(["Echo"]), [{ method: "POST", path: "/echo", responseType: "EchoResponse" }]),
    /references type `?'?EchoResponse'?`?, which is not a table/,
  );
});

test("checkTypes accepts operations whose types are all in the schema", () => {
  assert.doesNotThrow(() =>
    checkTypes(schema(["Echo", "EchoOut"]), [
      { method: "POST", path: "/echo", requestType: "Echo", responseType: "EchoOut" },
    ]),
  );
});

test("checkTypes ignores a request type on a GET, which the client never emits", () => {
  assert.doesNotThrow(() =>
    checkTypes(schema(["Health"]), [{ method: "GET", path: "/health", requestType: "Ghost", responseType: "Health" }]),
  );
});

test("emitIndex re-exports the whole folder", () => {
  const out = emitIndex();
  assert.match(out, /export \* from ".\/types.js";/);
  assert.match(out, /export \* from ".\/client.js";/);
  assert.match(out, /export \* as codec from ".\/codec.js";/);
});
