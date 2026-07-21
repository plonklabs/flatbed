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

test("marks strings, tables, and vectors optional; scalars and enums required", () => {
  const out = emitTypes(
    table("Row", [
      field("name", { kind: "string" }),
      field("addr", { kind: "table", name: "Address" }),
      field("tags", { kind: "vector", element: { kind: "string" } }),
      field("age", { kind: "scalar", scalar: "int32" }),
      field("level", { kind: "enum", name: "Severity" }),
    ]),
  );
  assert.match(out, /name\?: string;/);
  assert.match(out, /addr\?: Address;/);
  assert.match(out, /tags\?: string\[\];/);
  assert.match(out, /age: number;/);
  assert.match(out, /level: Severity;/);
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
