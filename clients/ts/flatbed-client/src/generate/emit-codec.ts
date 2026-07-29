import type { FbsEnum, FbsField, FbsSchema, FbsTable, FbsType } from "./model.js";
import { HEADER } from "./util.js";
import { SCALAR_OPS, type ScalarOps } from "./wire.js";

/** Lookups the emitters share, gathered once from the schema. */
interface Ctx {
  readonly enums: ReadonlyMap<string, FbsEnum>;
}

const ctxOf = (schema: FbsSchema): Ctx => ({
  enums: new Map(schema.enums.map((e) => [e.name, e])),
});

/** The scalar ops backing a scalar/enum wire type (an enum → its underlying). */
const opsFor = (ctx: Ctx, t: FbsType): ScalarOps => {
  if (t.kind === "scalar") return SCALAR_OPS[t.scalar];
  if (t.kind !== "enum") throw new Error(`no scalar ops for wire kind ${t.kind}`);
  const en = ctx.enums.get(t.name);
  if (en === undefined) throw new Error(`unknown enum ${t.name}`);
  return SCALAR_OPS[en.underlying];
};

/** The `n` suffix a scalar/enum literal needs when it is 64-bit backed. */
const suffix = (ctx: Ctx, t: FbsType): string => (opsFor(ctx, t).zero.endsWith("n") ? "n" : "");

/**
 * The wire default a field is compared against (encode's omit-if-equal) and the
 * value it decodes to when absent, as TS literals. The reader already resolved
 * an enum's declared default to its integer value.
 */
const defaults = (ctx: Ctx, f: FbsField): { readonly encode: string; readonly decode: string } => {
  const { type: t, default: d } = f;
  if (t.kind === "scalar" && t.scalar === "bool") {
    const on = d.kind === "int" && d.value !== 0n;
    return { encode: on ? "1" : "0", decode: on ? "true" : "false" };
  }
  if (t.kind === "enum") {
    const lit = `${d.kind === "int" ? d.value : 0n}${suffix(ctx, t)}`;
    return { encode: lit, decode: lit };
  }
  if (d.kind === "none") {
    throw new Error(`unreachable: scalar/enum field '${f.name}' has no wire default`);
  }
  const lit = d.kind === "int" ? `${d.value}${suffix(ctx, t)}` : `${d.value}`;
  return { encode: lit, decode: lit };
};

// Wrap a vector's build statements in a multi-line IIFE returning the finished
// `builder.endVector()` offset. Callers emit the elements back-to-front (a
// FlatBuffer vector is written last element first).
const vectorIife = (stmts: readonly string[]): string =>
  "(() => {\n" +
  stmts.map((s) => `      ${s}\n`).join("") +
  "      return builder.endVector();\n" +
  "    })()";

const buildVector = (ctx: Ctx, expr: string, element: FbsType): string => {
  if (element.kind === "table") {
    return vectorIife([
      `const o = ${expr}.map((x) => encode${element.name}(builder, x));`,
      "builder.startVector(4, o.length, 4);",
      "[...o].reverse().forEach((off) => builder.addOffset(off));",
    ]);
  }
  if (element.kind === "string") {
    return vectorIife([
      `const o = ${expr}.map((x) => builder.createString(x));`,
      "builder.startVector(4, o.length, 4);",
      "[...o].reverse().forEach((off) => builder.addOffset(off));",
    ]);
  }
  const ops = opsFor(ctx, element);
  const elem = element.kind === "scalar" && element.scalar === "bool" ? "x ? 1 : 0" : "x";
  return vectorIife([
    `const a = ${expr};`,
    `builder.startVector(${ops.size}, a.length, ${ops.size});`,
    `[...a].reverse().forEach((x) => builder.${ops.vecAdd}(${elem}));`,
  ]);
};

/** `[prep lines, addField line]` for one field's encode. */
const encodeField = (ctx: Ctx, f: FbsField): readonly [string, string] => {
  const val = `value.${f.name}`;
  const off = `${f.name}Offset`;
  const offsetAdd = `  if (${off}) builder.addFieldOffset(${f.id}, ${off}, 0);\n`;
  const t = f.type;
  if (t.kind === "vector") {
    return [`  const ${off} = ${val} != null ? ${buildVector(ctx, val, t.element)} : 0;\n`, offsetAdd];
  }
  if (t.kind === "table") {
    return [`  const ${off} = ${val} != null ? encode${t.name}(builder, ${val}) : 0;\n`, offsetAdd];
  }
  if (t.kind === "string") {
    return [`  const ${off} = ${val} != null ? builder.createString(${val}) : 0;\n`, offsetAdd];
  }
  const ops = opsFor(ctx, t);
  const arg = t.kind === "scalar" && t.scalar === "bool" ? `${val} ? 1 : 0` : val;
  return ["", `  builder.${ops.addField}(${f.id}, ${arg}, ${defaults(ctx, f).encode});\n`];
};

// The vtable needs one slot per field id (not per field): gapped explicit ids
// allocate up to `max id + 1` slots.
const slotCount = (t: FbsTable): number =>
  t.fields.reduce((max, f) => Math.max(max, f.id + 1), 0);

const encodeTable = (ctx: Ctx, t: FbsTable): string => {
  const [preps, adds] = t.fields
    .map((f) => encodeField(ctx, f))
    .reduce<[string, string]>(([p, a], [fp, fa]) => [p + fp, a + fa], ["", ""]);
  return (
    `export function encode${t.name}(builder: flatbuffers.Builder, value: ${t.name}): flatbuffers.Offset {\n` +
    preps +
    `  builder.startObject(${slotCount(t)});\n` +
    adds +
    "  return builder.endObject();\n}\n\n" +
    `export function encode${t.name}Root(value: ${t.name}): Uint8Array {\n` +
    "  const builder = new flatbuffers.Builder();\n" +
    `  builder.finish(encode${t.name}(builder, value));\n` +
    "  return builder.asUint8Array();\n}\n\n"
  );
};

/** The TS expression reading the i-th vector element from `base`. */
const decodeElement = (ctx: Ctx, element: FbsType): string => {
  if (element.kind === "table") return `decode${element.name}(bb, bb.__indirect(base + i * 4))`;
  if (element.kind === "string") return "bb.__string(base + i * 4) as string";
  if (element.kind === "scalar" && element.scalar === "bool") return "bb.readInt8(base + i) !== 0";
  const ops = opsFor(ctx, element);
  const read = `bb.${ops.read}(base + i * ${ops.size})`;
  return element.kind === "enum" ? `${read} as ${element.name}` : read;
};

/** The TS expression reading one field, given `{name}_o` (its vtable offset). */
const decodeField = (ctx: Ctx, f: FbsField): string => {
  const o = `${f.name}_o`;
  const t = f.type;
  if (t.kind === "vector") {
    const tsElem = decodeElementType(t.element);
    return (
      `${o}\n` +
      `      ? (() => {\n` +
      `          const len = bb.__vector_len(pos + ${o});\n` +
      `          const base = bb.__vector(pos + ${o});\n` +
      `          return Array.from({ length: len }, (_, i): ${tsElem} => ${decodeElement(ctx, t.element)});\n` +
      `        })()\n` +
      `      : undefined`
    );
  }
  if (t.kind === "table") return `${o} ? decode${t.name}(bb, bb.__indirect(pos + ${o})) : undefined`;
  if (t.kind === "string") return `${o} ? (bb.__string(pos + ${o}) as string) : undefined`;
  const dec = defaults(ctx, f).decode;
  if (t.kind === "scalar" && t.scalar === "bool") return `${o} ? bb.readInt8(pos + ${o}) !== 0 : ${dec}`;
  const ops = opsFor(ctx, t);
  if (t.kind === "enum") return `(${o} ? bb.${ops.read}(pos + ${o}) : ${dec}) as ${t.name}`;
  return `${o} ? bb.${ops.read}(pos + ${o}) : ${dec}`;
};

/** The TS type of a vector element, for the decoded array's annotation. */
const decodeElementType = (t: FbsType): string => {
  if (t.kind === "table" || t.kind === "enum") return t.name;
  if (t.kind === "string") return "string";
  if (t.kind === "scalar") return SCALAR_OPS[t.scalar].ts;
  return "unknown";
};

const decodeTable = (ctx: Ctx, t: FbsTable): string => {
  const offsets = t.fields
    .map((f) => `  const ${f.name}_o = bb.__offset(pos, ${4 + f.id * 2});\n`)
    .join("");
  const fields = t.fields.map((f) => `    ${f.name}: ${decodeField(ctx, f)},\n`).join("");
  return (
    `export function decode${t.name}(bb: flatbuffers.ByteBuffer, pos: number): ${t.name} {\n` +
    offsets +
    "  return {\n" +
    fields +
    "  };\n}\n\n" +
    `export function decode${t.name}Root(bytes: Uint8Array): ${t.name} {\n` +
    "  const bb = new flatbuffers.ByteBuffer(bytes);\n" +
    `  return decode${t.name}(bb, bb.__indirect(bb.position()));\n}\n\n`
  );
};

// An enum only appears in the codec as an `as Name` cast, so import only those a
// field actually uses; an unreferenced enum would be a dead import.
const referencedEnums = (schema: FbsSchema): ReadonlySet<string> => {
  const names = new Set<string>();
  const visit = (t: FbsType): void => {
    if (t.kind === "enum") names.add(t.name);
    if (t.kind === "vector") visit(t.element);
  };
  schema.tables.forEach((table) => table.fields.forEach((f) => visit(f.type)));
  return names;
};

/** Emit per-table encode/decode functions over the `flatbuffers` runtime. */
export const emitCodec = (schema: FbsSchema): string => {
  const ctx = ctxOf(schema);
  const used = referencedEnums(schema);
  const typeList = [
    ...schema.tables.map((t) => t.name),
    ...schema.enums.filter((e) => used.has(e.name)).map((e) => e.name),
  ].join(", ");
  return (
    HEADER +
    // Only table encode/decode uses the runtime; with no tables the import would
    // be unused and `noUnusedLocals` would reject the generated file.
    (schema.tables.length > 0 ? 'import * as flatbuffers from "flatbuffers";\n' : "") +
    (typeList.length > 0 ? `import type { ${typeList} } from "./types.js";\n` : "") +
    "\n" +
    schema.tables.map((t) => encodeTable(ctx, t) + decodeTable(ctx, t)).join("")
  );
};
