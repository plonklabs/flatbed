import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { emitJson } from "./emit-json.js";
import { emitTypes } from "./emit-types.js";
import type { CodecRoots, FbsField, FbsSchema } from "./model.js";
import { readBfbs } from "./read-bfbs.js";

const schema = readBfbs(readFileSync(fileURLToPath(new URL("./__fixtures__/test.bfbs", import.meta.url))));

// Emit every table's full encode+decode surface, so the round-trip assertions
// here reach each type's `…Json` functions regardless of direction.
const bothRoots = (s: FbsSchema): CodecRoots => {
  const all = new Set(s.tables.map((t) => t.name));
  return { encodeRoots: all, decodeRoots: all };
};

/** Compile-and-load the generated JSON codec once; it imports enum values as runtime values, so the type module must sit alongside it. */
type Codec = Record<string, (arg: never) => never>;
const codec: Promise<Codec> = (() => {
  const dir = fileURLToPath(new URL("../../node_modules/.cache/flatbed-json-test", import.meta.url));
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/types.ts`, emitTypes(schema));
  writeFileSync(`${dir}/json-codec.ts`, emitJson(schema, bothRoots(schema)));
  return import(`${dir}/json-codec.ts`) as Promise<Codec>;
})();

test("emits only the direction each body type is actually used in", () => {
  const scalar: FbsField = { name: "x", id: 0, type: { kind: "scalar", scalar: "int32" }, default: { kind: "int", value: 0n } };
  const s: FbsSchema = {
    tables: [
      { name: "Req", fields: [scalar] },
      { name: "Resp", fields: [scalar] },
    ],
    enums: [],
  };
  const out = emitJson(s, { encodeRoots: new Set(["Req"]), decodeRoots: new Set(["Resp"]) });
  assert.match(out, /export function encodeReqJson\(/);
  assert.doesNotMatch(out, /decodeReqJson\b/);
  assert.match(out, /export function decodeRespJson\(/);
  assert.doesNotMatch(out, /encodeRespJson\b/);
});

const Severity = { Info: 0, Warning: 1, Error: 2 } as const;

const roundTrip = <T>(name: string, value: T): Promise<void> =>
  codec.then((c) => {
    const encode = c[`encode${name}Json`] as unknown as (v: T) => Uint8Array;
    const decode = c[`decode${name}Json`] as unknown as (b: Uint8Array) => T;
    assert.deepEqual(decode(encode(value)), value);
  });

const wire = (name: string, value: unknown): Promise<Record<string, unknown>> =>
  codec.then((c) => {
    const encode = c[`encode${name}Json`] as unknown as (v: unknown) => Uint8Array;
    return JSON.parse(new TextDecoder().decode(encode(value)));
  });

test("round-trips scalars, strings, and 64-bit ints (within the safe range)", () =>
  roundTrip("TestResponse", { message: "pong", value: 42n, success: true }));

test("round-trips a nested table", () =>
  roundTrip("UserRequest", { name: "Ada", age: 36, address: { street: "1 Way", city: "London", zip_code: 12345 } }));

test("round-trips vectors of tables and strings", () =>
  roundTrip("AddressBook", {
    owner: "Ada",
    addresses: [{ street: "1 Way", city: "London", zip_code: 1 }],
    contact_names: ["Bob", "Carol"],
  }));

test("round-trips an enum and a vector of enums", () =>
  roundTrip("LogEvent", {
    message: "disk full",
    severity: Severity.Error,
    history: [Severity.Info, Severity.Warning, Severity.Error],
  }));

test("enums serialize as variant-name strings on the JSON wire", () =>
  wire("LogEvent", { message: "x", severity: Severity.Error, history: [Severity.Info, Severity.Warning] }).then((w) => {
    assert.equal(w.severity, "Error");
    assert.deepEqual(w.history, ["Info", "Warning"]);
  }));

test("64-bit ints serialize as JSON numbers on the wire", () =>
  wire("TestResponse", { message: "x", value: 42n, success: true }).then((w) => {
    assert.equal(typeof w.value, "number");
    assert.equal(w.value, 42);
  }));

test("absent optional fields (string, table, vector) decode to undefined", () =>
  codec.then((c) => {
    const enc = (o: unknown): Uint8Array => new TextEncoder().encode(JSON.stringify(o));
    const testResp = c.decodeTestResponseJson as unknown as (b: Uint8Array) => { message?: string };
    const user = c.decodeUserRequestJson as unknown as (b: Uint8Array) => { address?: unknown };
    const book = c.decodeAddressBookJson as unknown as (b: Uint8Array) => { addresses?: unknown; contact_names?: unknown };
    assert.equal(testResp(enc({ value: 1, success: true })).message, undefined);
    assert.equal(user(enc({ name: "Ada", age: 36 })).address, undefined);
    const b = book(enc({ owner: "Ada" }));
    assert.equal(b.addresses, undefined);
    assert.equal(b.contact_names, undefined);
  }));

test("an absent 64-bit field decodes to its default instead of throwing", () =>
  // A server that omits a zero-valued field must not make BigInt(undefined) throw.
  codec.then((c) => {
    const decode = c.decodeTestResponseJson as unknown as (b: Uint8Array) => { value: bigint; success: boolean };
    const bytes = new TextEncoder().encode(JSON.stringify({ message: "x", success: true }));
    assert.equal(decode(bytes).value, 0n);
  }));

test("64-bit values above 2^53 lose precision on the JSON path", () =>
  // The server encodes 64-bit as a JSON number too — precision loss here is a limit of the JSON number type.
  codec.then((c) => {
    const encode = c.encodeTestResponseJson as unknown as (v: unknown) => Uint8Array;
    const decode = c.decodeTestResponseJson as unknown as (b: Uint8Array) => { value: bigint };
    const big = 9007199254740993n;
    assert.notEqual(decode(encode({ message: "x", value: big, success: true })).value, big);
  }));
