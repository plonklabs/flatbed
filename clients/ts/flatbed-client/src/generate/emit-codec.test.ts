import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { emitCodec } from "./emit-codec.js";
import { readBfbs } from "./read-bfbs.js";

const schema = readBfbs(readFileSync(fileURLToPath(new URL("./__fixtures__/test.bfbs", import.meta.url))));

/**
 * Compile-and-load the generated codec once. It's written under `node_modules`
 * so its bare `flatbuffers` import resolves; `tsx` compiles it on import, and
 * its `import type … from "./types.js"` is erased at runtime.
 */
type Codec = Record<string, (arg: never) => never>;
const codec: Promise<Codec> = (() => {
  const dir = fileURLToPath(new URL("../../node_modules/.cache/flatbed-codec-test", import.meta.url));
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/codec.ts`, emitCodec(schema));
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
