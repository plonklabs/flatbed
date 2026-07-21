import type { FbsEnum, FbsSchema, FbsTable, FbsType, ScalarType } from "./model.js";
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
      return t.name;
    case "vector":
      return `${tsType(t.element)}[]`;
  }
};

// Strings, tables, and vectors can be absent on the wire → optional in TS.
const isWireOptional = (t: FbsType): boolean =>
  t.kind === "vector" || t.kind === "string" || t.kind === "table";

const emitEnum = (e: FbsEnum): string =>
  `export enum ${e.name} {\n` +
  e.members.map((m) => `  ${m.name} = ${m.value},\n`).join("") +
  "}\n\n";

const emitInterface = (t: FbsTable): string =>
  `export interface ${t.name} {\n` +
  t.fields.map((f) => `  ${f.name}${isWireOptional(f.type) ? "?" : ""}: ${tsType(f.type)};\n`).join("") +
  "}\n\n";

/** Emit `types.ts`: numeric enums (true wire values) + table interfaces. */
export const emitTypes = (schema: FbsSchema): string =>
  HEADER + schema.enums.map(emitEnum).join("") + schema.tables.map(emitInterface).join("");
