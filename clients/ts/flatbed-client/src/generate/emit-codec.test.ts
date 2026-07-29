import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { emitCodec } from "./emit-codec.js";
import type { CodecRoots, FbsField, FbsSchema } from "./model.js";
import { readBfbs } from "./read-bfbs.js";

const schema = readBfbs(readFileSync(fileURLToPath(new URL("./__fixtures__/test.bfbs", import.meta.url))));

// Treat every table as both a request and a response body so the import and
// round-trip assertions exercise the full encode+decode surface.
const bothRoots = (s: FbsSchema): CodecRoots => {
  const all = new Set(s.tables.map((t) => t.name));
  return { encodeRoots: all, decodeRoots: all };
};

test("an enum no field references is not imported (would trip noUnusedLocals)", () => {
  const s: FbsSchema = {
    tables: [
      { name: "T", fields: [{ name: "x", id: 0, type: { kind: "scalar", scalar: "int32" }, default: { kind: "int", value: 0n } }] },
    ],
    enums: [{ name: "Unused", underlying: "int8", members: [{ name: "A", value: 0n }] }],
  };
  const out = emitCodec(s, bothRoots(s));
  assert.doesNotMatch(out, /Unused/);
  assert.match(out, /import type \{ T \} from ".\/types.js";/);
});

test("omits the flatbuffers import when the schema has no tables", () => {
  // With no tables the runtime is unused; emitting the import would trip noUnusedLocals.
  const out = emitCodec({ tables: [], enums: [] }, { encodeRoots: new Set(), decodeRoots: new Set() });
  assert.doesNotMatch(out, /import \* as flatbuffers/);
});

test("an enum a field references is imported", () => {
  const s: FbsSchema = {
    tables: [
      { name: "T", fields: [{ name: "e", id: 0, type: { kind: "enum", name: "E" }, default: { kind: "int", value: 0n } }] },
    ],
    enums: [{ name: "E", underlying: "int8", members: [{ name: "A", value: 0n }] }],
  };
  assert.match(emitCodec(s, bothRoots(s)), /import type \{ T, E \} from ".\/types.js";/);
});

test("an enum field of an encode-only table is not imported (encode writes the raw int)", () => {
  const s: FbsSchema = {
    tables: [
      { name: "T", fields: [{ name: "e", id: 0, type: { kind: "enum", name: "E" }, default: { kind: "int", value: 0n } }] },
    ],
    enums: [{ name: "E", underlying: "int8", members: [{ name: "A", value: 0n }] }],
  };
  // Only encoded: the FlatBuffer encode path writes E's underlying integer and
  // never names the enum, so E stays out of the type import (a `decode` cast is
  // the only place the codec names an enum).
  const out = emitCodec(s, { encodeRoots: new Set(["T"]), decodeRoots: new Set() });
  assert.match(out, /import type \{ T \} from ".\/types.js";/);
  assert.doesNotMatch(out, /\bE\b/);
});

const scalarField = (name: string): FbsField => ({
  name,
  id: 0,
  type: { kind: "scalar", scalar: "int32" },
  default: { kind: "int", value: 0n },
});

test("emits only the direction each body type is actually used in", () => {
  const s: FbsSchema = {
    tables: [
      { name: "Req", fields: [scalarField("x")] },
      { name: "Resp", fields: [scalarField("y")] },
      { name: "ServerOnly", fields: [scalarField("z")] },
    ],
    enums: [],
  };
  const out = emitCodec(s, { encodeRoots: new Set(["Req"]), decodeRoots: new Set(["Resp"]) });
  // A request body is encoded, never decoded, by a client.
  assert.match(out, /export function encodeReq\(/);
  assert.match(out, /export function encodeReqRoot\(/);
  assert.doesNotMatch(out, /decodeReq\b/);
  // A response body is decoded, never encoded.
  assert.match(out, /export function decodeResp\(/);
  assert.match(out, /export function decodeRespRoot\(/);
  assert.doesNotMatch(out, /encodeResp\b/);
  // A table on neither side of any operation is not emitted at all.
  assert.doesNotMatch(out, /ServerOnly/);
});

test("a nested table gets its parent's direction but no Root of its own", () => {
  const s: FbsSchema = {
    tables: [
      { name: "Req", fields: [{ name: "n", id: 0, type: { kind: "table", name: "Nested" }, default: { kind: "none" } }] },
      { name: "Nested", fields: [scalarField("x")] },
    ],
    enums: [],
  };
  const out = emitCodec(s, { encodeRoots: new Set(["Req"]), decodeRoots: new Set() });
  assert.match(out, /export function encodeNested\(/); // reachable from a request body
  assert.doesNotMatch(out, /encodeNestedRoot\b/); // nested, so never a top-level body
  assert.doesNotMatch(out, /decodeNested\b/); // no response body reaches it
});

/**
 * Compile-and-load the generated codec once. It's written under `node_modules`
 * so its bare `flatbuffers` import resolves — the package's own deps must be
 * installed (`npm ci` runs before `npm test` in CI). `tsx` compiles it on
 * import, and its `import type … from "./types.js"` is erased at runtime.
 */
type Codec = Record<string, (arg: never) => never>;
const codec: Promise<Codec> = (() => {
  const dir = fileURLToPath(new URL("../../node_modules/.cache/flatbed-codec-test", import.meta.url));
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/codec.ts`, emitCodec(schema, bothRoots(schema)));
  return import(`${dir}/codec.ts`) as Promise<Codec>;
})();

const roundTrip = <T>(name: string, value: T): Promise<void> =>
  codec.then((c) => {
    const encode = c[`encode${name}Root`] as unknown as (v: T) => Uint8Array;
    const decode = c[`decode${name}Root`] as unknown as (b: Uint8Array) => T;
    const bytes = encode(value);
    assert.ok(bytes.length > 0, "encodes to a non-empty buffer");
    assert.deepEqual(decode(bytes), value);
  });

test("round-trips scalars, strings, and a bool", () =>
  roundTrip("TestResponse", { message: "hi", value: 42n, success: true }));

test("round-trips declared defaults (present and omitted values)", () =>
  roundTrip("Defaulted", { count: 7, flag: false, ratio: 2.5, level: 2 }));

test("omitted scalars decode to their declared defaults", () =>
  codec.then((c) => {
    const encode = c.encodeDefaultedRoot as unknown as (v: unknown) => Uint8Array;
    const decode = c.decodeDefaultedRoot as unknown as (b: Uint8Array) => Record<string, unknown>;
    // Encode a value equal to every declared default; the codec omits them,
    // and decode must recover the declared defaults (25, true, 1.5, Warning=1).
    const bytes = encode({ count: 25, flag: true, ratio: 1.5, level: 1 });
    assert.deepEqual(decode(bytes), { count: 25, flag: true, ratio: 1.5, level: 1 });
  }));

test("round-trips a nested table", () =>
  roundTrip("UserRequest", {
    name: "Ada",
    age: 36,
    address: { street: "1 Rue", city: "Paris", zip_code: 75001 },
  }));

test("round-trips vectors of tables, strings, and enums", () =>
  Promise.all([
    roundTrip("AddressBook", {
      owner: "Ada",
      addresses: [{ street: "a", city: "b", zip_code: 1 }],
      contact_names: ["x", "y"],
    }),
    roundTrip("LogEvent", { message: "boom", severity: 2, history: [0, 1, 2] }),
  ]).then(() => undefined));
