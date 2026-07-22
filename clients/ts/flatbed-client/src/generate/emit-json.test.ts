import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { emitJson } from "./emit-json.js";
import { emitTypes } from "./emit-types.js";
import { readBfbs } from "./read-bfbs.js";

const schema = readBfbs(readFileSync(fileURLToPath(new URL("./__fixtures__/test.bfbs", import.meta.url))));

/** Compile-and-load the generated JSON codec once; it imports enum values as runtime values, so the type module must sit alongside it. */
type Codec = Record<string, (arg: never) => never>;
const codec: Promise<Codec> = (() => {
  const dir = fileURLToPath(new URL("../../node_modules/.cache/flatbed-json-test", import.meta.url));
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/types.ts`, emitTypes(schema));
  writeFileSync(`${dir}/json-codec.ts`, emitJson(schema));
  return import(`${dir}/json-codec.ts`) as Promise<Codec>;
})();

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

test("64-bit values above 2^53 lose precision on the JSON path", () =>
  // The server encodes 64-bit as a JSON number too — precision loss here is a limit of the JSON number type.
  codec.then((c) => {
    const encode = c.encodeTestResponseJson as unknown as (v: unknown) => Uint8Array;
    const decode = c.decodeTestResponseJson as unknown as (b: Uint8Array) => { value: bigint };
    const big = 9007199254740993n; // 2^53 + 1
    assert.notEqual(decode(encode({ message: "x", value: big, success: true })).value, big);
  }));
