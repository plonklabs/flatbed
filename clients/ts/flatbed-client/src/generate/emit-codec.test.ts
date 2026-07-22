import assert from "node:assert/strict";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import { emitCodec } from "./emit-codec.js";
import type { FbsSchema } from "./model.js";
import { readBfbs } from "./read-bfbs.js";

const schema = readBfbs(readFileSync(fileURLToPath(new URL("./__fixtures__/test.bfbs", import.meta.url))));

test("an enum no field references is not imported (would trip noUnusedLocals)", () => {
  const s: FbsSchema = {
    tables: [
      { name: "T", fields: [{ name: "x", id: 0, type: { kind: "scalar", scalar: "int32" }, default: { kind: "int", value: 0n } }] },
    ],
    enums: [{ name: "Unused", underlying: "int8", members: [{ name: "A", value: 0n }] }],
  };
  const out = emitCodec(s);
  assert.doesNotMatch(out, /Unused/);
  assert.match(out, /import type \{ T \} from ".\/types.js";/);
});

test("omits the flatbuffers import when the schema has no tables", () => {
  // With no tables the runtime is unused; emitting the import would trip noUnusedLocals.
  const out = emitCodec({ tables: [], enums: [] });
  assert.doesNotMatch(out, /import \* as flatbuffers/);
});

test("an enum a field references is imported", () => {
  const s: FbsSchema = {
    tables: [
      { name: "T", fields: [{ name: "e", id: 0, type: { kind: "enum", name: "E" }, default: { kind: "int", value: 0n } }] },
    ],
    enums: [{ name: "E", underlying: "int8", members: [{ name: "A", value: 0n }] }],
  };
  assert.match(emitCodec(s), /import type \{ T, E \} from ".\/types.js";/);
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
