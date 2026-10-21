import assert from "node:assert/strict";
import { test } from "node:test";

import { emitTypes } from "./emit-types.js";
import type { FbsField, FbsSchema, FbsType } from "./model.js";

const field = (name: string, type: FbsType): FbsField => ({ name, id: 0, type, default: { kind: "none" } });

const table = (name: string, fields: readonly FbsField[]): FbsSchema => ({
  tables: [{ name, fields }],
  enums: [],
});

test("maps scalar wire types to their TS types", () => {
  const out = emitTypes(
    table("Row", [
      field("count", { kind: "scalar", scalar: "int32" }),
      field("big", { kind: "scalar", scalar: "uint64" }),
      field("flag", { kind: "scalar", scalar: "bool" }),
      field("ratio", { kind: "scalar", scalar: "float64" }),
    ]),
  );
  assert.match(out, /count: number;/);
  assert.match(out, /big: bigint;/); // 64-bit → bigint
  assert.match(out, /flag: boolean;/);
  assert.match(out, /ratio: number;/);
});

test("emits readonly fields, optional for strings/tables/vectors and readonly arrays", () => {
  const out = emitTypes(
    table("Row", [
      field("name", { kind: "string" }),
      field("addr", { kind: "table", name: "Address" }),
      field("tags", { kind: "vector", element: { kind: "string" } }),
      field("age", { kind: "scalar", scalar: "int32" }),
      field("level", { kind: "enum", name: "Severity" }),
    ]),
  );
  assert.match(out, /readonly name\?: string;/);
  assert.match(out, /readonly addr\?: Address;/);
  assert.match(out, /readonly tags\?: readonly string\[\];/);
  assert.match(out, /readonly age: number;/);
  assert.match(out, /readonly level: Severity;/);
});

test("emits numeric enums with their wire values", () => {
  const out = emitTypes({
    tables: [],
    enums: [{ name: "Severity", underlying: "int8", members: [
      { name: "Info", value: 0n },
      { name: "Error", value: 2n },
    ] }],
  });
  assert.match(out, /export enum Severity \{/);
  assert.match(out, /Info = 0,/);
  assert.match(out, /Error = 2,/);
});

test("a bit_flags enum is a flag-name type and its field a list of names", () => {
  const out = emitTypes({
    tables: [
      {
        name: "Row",
        fields: [
          field("traits", { kind: "flags", name: "Traits" }),
          field("history", { kind: "vector", element: { kind: "flags", name: "Traits" } }),
        ],
      },
    ],
    enums: [{ name: "Traits", underlying: "uint64", bitFlags: true, members: [
      { name: "A", value: 1n },
      { name: "B", value: 2n },
    ] }],
  });
  assert.match(out, /export type Traits = "A" \| "B";/);
  assert.match(out, /readonly traits: readonly Traits\[\];/);
  assert.match(out, /readonly history\?: readonly \(readonly Traits\[\]\)\[\];/);
});

test("a union is a tagged type and its field optional", () => {
  const out = emitTypes({
    tables: [{ name: "Row", fields: [field("ref", { kind: "union", name: "Ref" })] }],
    enums: [],
    unions: [{ name: "Ref", members: [
      { name: "Mail", table: "MailRef", value: 1 },
      { name: "File", table: "FileRef", value: 2 },
    ] }],
  });
  assert.match(
    out,
    /export type Ref =\n {2}\| \{ readonly type: "Mail"; readonly value: MailRef \}\n {2}\| \{ readonly type: "File"; readonly value: FileRef \};/,
  );
  assert.match(out, /readonly ref\?: Ref;/);
});
