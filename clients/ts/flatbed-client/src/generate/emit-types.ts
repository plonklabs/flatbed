import type { FbsEnum, FbsSchema, FbsTable, FbsType, FbsUnion, ScalarType } from "./model.js";
import { HEADER } from "./util.js";

const tsScalar = (s: ScalarType): string =>
  s === "bool" ? "boolean" : s === "int64" || s === "uint64" ? "bigint" : "number";

/** The TS type for a field's wire type; a vector nests its element type. */
const tsType = (t: FbsType): string => {
  switch (t.kind) {
    case "scalar":
      return tsScalar(t.scalar);
    case "string":
      return "string";
    case "enum":
    case "table":
    case "union":
      return t.name;
    case "flags":
      return `readonly ${t.name}[]`;
    case "vector":
      return t.element.kind === "flags" ? `readonly (${tsType(t.element)})[]` : `readonly ${tsType(t.element)}[]`;
  }
};

// Strings, tables, unions, and vectors can be absent on the wire → optional in TS.
const isWireOptional = (t: FbsType): boolean =>
  t.kind === "vector" || t.kind === "string" || t.kind === "table" || t.kind === "union";

// A plain enum is a numeric enum of its true wire values; a `bit_flags` enum
// names one flag, and a field holds the list of its set flags.
const emitEnum = (e: FbsEnum): string =>
  e.bitFlags === true
    ? `export type ${e.name} = ${e.members.map((m) => JSON.stringify(m.name)).join(" | ")};\n\n`
    : `export enum ${e.name} {\n` + e.members.map((m) => `  ${m.name} = ${m.value},\n`).join("") + "}\n\n";

const emitUnion = (u: FbsUnion): string =>
  `export type ${u.name} =\n` +
  u.members.map((m) => `  | { readonly type: ${JSON.stringify(m.name)}; readonly value: ${m.table} }`).join("\n") +
  ";\n\n";

const emitInterface = (t: FbsTable): string =>
  `export interface ${t.name} {\n` +
  t.fields.map((f) => `  readonly ${f.name}${isWireOptional(f.type) ? "?" : ""}: ${tsType(f.type)};\n`).join("") +
  "}\n\n";

/** Emit numeric enums (true wire values), flag-name types, unions, and table interfaces. */
export const emitTypes = (schema: FbsSchema): string =>
  HEADER +
  schema.enums.map(emitEnum).join("") +
  (schema.unions ?? []).map(emitUnion).join("") +
  schema.tables.map(emitInterface).join("");
