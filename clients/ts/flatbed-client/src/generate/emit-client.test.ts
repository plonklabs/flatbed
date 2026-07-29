import assert from "node:assert/strict";
import { test } from "node:test";

import { checkNames, checkTypes, emitClient, emitIndex, methodName } from "./emit-client.js";
import type { FbsSchema, Operation } from "./model.js";

/** An Operation with `supportsJson` defaulted off, so tests opt into JSON. */
const op = (o: Omit<Operation, "supportsJson"> & { supportsJson?: boolean }): Operation => ({
  supportsJson: false,
  ...o,
});

test("methodName camelCases an operationId", () => {
  assert.equal(methodName(op({ method: "POST", path: "/x", operationId: "CreateUser" })), "createUser");
});

test("methodName derives <method><PascalPath> without an operationId", () => {
  assert.equal(methodName(op({ method: "GET", path: "/users/{id}" })), "getUsersById");
});

test("emitClient generates a createFlatbedClient factory", () => {
  const out = emitClient([
    op({ method: "POST", path: "/echo", operationId: "echo", requestType: "EchoRequest", responseType: "EchoResponse" }),
  ]);
  assert.match(out, /import \{ request, type ClientConfig, FLATBUFFERS_CONTENT_TYPE \} from "@plonklabs\/flatbed-client";/);
  assert.match(out, /export const createFlatbedClient = \(config: ClientConfig\) => \(\{/);
  assert.match(out, /echo: \(args: \{ body: EchoRequest \}\): Promise<EchoResponse> =>/);
  assert.match(out, /request\(config, "POST", "\/echo", codec\.encodeEchoRequestRoot\(args\.body\), codec\.decodeEchoResponseRoot, FLATBUFFERS_CONTENT_TYPE\)/);
});

test("a JSON-capable operation offers { as } defaulting to FlatBuffer", () => {
  const out = emitClient([
    op({ method: "POST", path: "/echo", operationId: "echo", requestType: "EchoRequest", responseType: "EchoResponse", supportsJson: true }),
  ]);
  assert.match(out, /echo: \(args: \{ body: EchoRequest \}, opts\?: \{ as\?: "json" \| "flatbuffer" \}\): Promise<EchoResponse> =>/);
  assert.match(out, /opts\?\.as === "json"\n\s*\? request\(config, "POST", "\/echo", json\.encodeEchoRequestJson\(args\.body\), json\.decodeEchoResponseJson, JSON_CONTENT_TYPE\)/);
  assert.match(out, /\n\s*: request\(config, "POST", "\/echo", codec\.encodeEchoRequestRoot\(args\.body\), codec\.decodeEchoResponseRoot, FLATBUFFERS_CONTENT_TYPE\)/);
  assert.match(out, /import \* as json from ".\/json-codec.js";/);
  assert.match(out, /JSON_CONTENT_TYPE/);
});

test("a JSON-capable but untyped operation imports JSON_CONTENT_TYPE but not the json codec", () => {
  const out = emitClient([op({ method: "GET", path: "/ping", supportsJson: true })]);
  assert.match(out, /JSON_CONTENT_TYPE/);
  assert.doesNotMatch(out, /import \* as json/);
});

test("a JSON-capable operation with no args takes only opts", () => {
  const out = emitClient([op({ method: "GET", path: "/health", responseType: "Health", supportsJson: true })]);
  assert.match(out, /getHealth: \(opts\?: \{ as\?: "json" \| "flatbuffer" \}\): Promise<Health> =>/);
});

test("a GET with a path param takes { pathParams } and no body", () => {
  const out = emitClient([op({ method: "GET", path: "/users/{id}", responseType: "User" })]);
  assert.match(out, /getUsersById: \(args: \{ pathParams: \{ id: string \} \}\): Promise<User> =>/);
  assert.match(out, /encodeURIComponent\(args\.pathParams\.id\)/);
  assert.match(out, /new Uint8Array\(\)/);
});

test("a JSON-capable PUT with a body and a path param takes both keys and opts", () => {
  const out = emitClient([
    op({ method: "PUT", path: "/users/{id}", requestType: "UserPatch", responseType: "User", supportsJson: true }),
  ]);
  assert.match(out, /putUsersById: \(args: \{ body: UserPatch; pathParams: \{ id: string \} \}, opts\?: \{ as\?: "json" \| "flatbuffer" \}\): Promise<User> =>/);
  assert.match(out, /json\.encodeUserPatchJson\(args\.body\)/);
  assert.match(out, /encodeURIComponent\(args\.pathParams\.id\)/);
});

test("a PUT with a body and a path param takes both keys", () => {
  const out = emitClient([op({ method: "PUT", path: "/users/{id}", requestType: "UserPatch", responseType: "User" })]);
  assert.match(out, /putUsersById: \(args: \{ body: UserPatch; pathParams: \{ id: string \} \}\): Promise<User> =>/);
  assert.match(out, /codec\.encodeUserPatchRoot\(args\.body\)/);
});

test("escapes an unsafe character in the path so the generated literal is valid", () => {
  const out = emitClient([op({ method: "GET", path: '/a"b', responseType: "R" })]);
  assert.ok(out.includes(JSON.stringify('/a"b'))); // quote is escaped, not embedded raw
});

test("a brace inside a larger segment stays literal, matching pathParams", () => {
  // `{id}` doesn't fill a whole segment, so pathParams sees no param → no `args`.
  const out = emitClient([op({ method: "GET", path: "/files/{id}.json", responseType: "R" })]);
  assert.match(out, /getFilesIdJson: \(\): Promise<R> =>/);
  assert.doesNotMatch(out, /args\.pathParams/);
  assert.ok(out.includes(JSON.stringify("{id}")));
});

test("an operation with neither body nor path params takes no argument", () => {
  const out = emitClient([op({ method: "GET", path: "/health", responseType: "Health" })]);
  assert.match(out, /getHealth: \(\): Promise<Health> =>/);
});

test("no codec/type imports when no operation is typed", () => {
  const out = emitClient([op({ method: "GET", path: "/ping" })]);
  assert.doesNotMatch(out, /import \* as codec/);
  assert.doesNotMatch(out, /import type/);
  assert.match(out, /getPing: \(\): Promise<Uint8Array> =>/);
});

test("a spec with no operations imports only ClientConfig", () => {
  const out = emitClient([]);
  assert.match(out, /import \{ type ClientConfig \} from "@plonklabs\/flatbed-client";/);
  assert.doesNotMatch(out, /FLATBUFFERS_CONTENT_TYPE/);
});

test("checkNames rejects duplicate derived method names", () => {
  assert.throws(
    () =>
      checkNames([
        op({ method: "GET", path: "/x", operationId: "dup" }),
        op({ method: "POST", path: "/y", operationId: "dup", requestType: "R" }),
      ]),
    /same client method `dup`/,
  );
});

test("checkNames rejects an operationId that derives an invalid identifier", () => {
  // "3D" camelCases to "3d" — a leading digit is not a valid TS identifier.
  assert.throws(
    () => checkNames([op({ method: "GET", path: "/x", operationId: "3D" })]),
    /derives the client method name `3d`, which is not a valid TypeScript identifier/,
  );
});

test("checkNames rejects duplicate path-param keys in a path", () => {
  assert.throws(
    () => checkNames([op({ method: "GET", path: "/users/{id}/posts/{id}" })]),
    /two path parameters that map to the key `id`/,
  );
});

const schema = (names: readonly string[]): FbsSchema => ({
  tables: names.map((name) => ({ name, fields: [] })),
  enums: [],
});

test("checkTypes rejects an operation referencing an unknown table", () => {
  assert.throws(
    () => checkTypes(schema(["Echo"]), [op({ method: "POST", path: "/echo", responseType: "EchoResponse" })]),
    /references type `?'?EchoResponse'?`?, which is not a table/,
  );
});

test("checkTypes accepts operations whose types are all in the schema", () => {
  assert.doesNotThrow(() =>
    checkTypes(schema(["Echo", "EchoOut"]), [
      op({ method: "POST", path: "/echo", requestType: "Echo", responseType: "EchoOut" }),
    ]),
  );
});

test("checkTypes ignores a request type on a GET, which the client never emits", () => {
  assert.doesNotThrow(() =>
    checkTypes(schema(["Health"]), [op({ method: "GET", path: "/health", requestType: "Ghost", responseType: "Health" })]),
  );
});

test("emitIndex re-exports the whole folder", () => {
  const out = emitIndex();
  assert.match(out, /export \* from ".\/types.js";/);
  assert.match(out, /export \* from ".\/client.js";/);
  assert.match(out, /export \* as codec from ".\/codec.js";/);
  assert.match(out, /export \* as json from ".\/json-codec.js";/);
});
