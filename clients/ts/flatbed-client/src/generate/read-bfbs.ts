import * as flatbuffers from "flatbuffers";

import {
  BaseType,
  type Enum,
  type Field,
  type Object_,
  Schema,
  type Type,
} from "./fbs-reflection/reflection.js";
import type {
  FbsDefault,
  FbsEnum,
  FbsField,
  FbsSchema,
  FbsTable,
  FbsType,
  FbsUnion,
  ScalarType,
} from "./model.js";
import { bareName, times } from "./util.js";

const INTEGER_SCALARS = new Map<BaseType, ScalarType>([
  [BaseType.Byte, "int8"],
  [BaseType.UByte, "uint8"],
  [BaseType.Short, "int16"],
  [BaseType.UShort, "uint16"],
  [BaseType.Int, "int32"],
  [BaseType.UInt, "uint32"],
  [BaseType.Long, "int64"],
  [BaseType.ULong, "uint64"],
]);

const SCALARS = new Map<BaseType, ScalarType>([
  ...INTEGER_SCALARS,
  [BaseType.Bool, "bool"],
  [BaseType.Float, "float32"],
  [BaseType.Double, "float64"],
]);

const hasAttribute = (decl: Enum | Object_, key: string): boolean =>
  times(decl.attributesLength(), (i) => decl.attributes(i)!).some((kv) => kv.key() === key);

// An integer base type with `index >= 0` references an enum; `Obj` references a
// table; `Union` references a union. Everything else is a plain scalar or
// string. (Structs/arrays aren't part of flatbed's supported subset.)
const scalarOrObj = (schema: Schema, base: BaseType, index: number): FbsType => {
  const en = index >= 0 && INTEGER_SCALARS.has(base) ? schema.enums(index) : null;
  if (en != null) {
    return { kind: hasAttribute(en, "bit_flags") ? "flags" : "enum", name: bareName(en.name() ?? "") };
  }
  if (base === BaseType.Union) {
    return { kind: "union", name: bareName(schema.enums(index)?.name() ?? "") };
  }
  const scalar = SCALARS.get(base);
  if (scalar !== undefined) {
    return { kind: "scalar", scalar };
  }
  if (base === BaseType.String) {
    return { kind: "string" };
  }
  if (base === BaseType.Obj) {
    const obj = schema.objects(index);
    // `Obj` covers tables and structs; structs are filtered from the emitted
    // model, so a struct field would reference a type that never gets defined.
    if (obj?.isStruct()) {
      throw new Error(`struct field type '${bareName(obj.name() ?? "")}' is not supported; use a table`);
    }
    return { kind: "table", name: bareName(obj?.name() ?? "") };
  }
  throw new Error(
    `unsupported FlatBuffer base type ${BaseType[base] ?? base}; structs and arrays are not supported`,
  );
};

const readType = (schema: Schema, type: Type): FbsType =>
  type.baseType() === BaseType.Vector
    ? { kind: "vector", element: scalarOrObj(schema, type.element(), type.index()) }
    : scalarOrObj(schema, type.baseType(), type.index());

const readDefault = (type: FbsType, field: Field): FbsDefault => {
  if (type.kind === "flags") {
    return { kind: "int", value: BigInt.asUintN(64, field.defaultInteger()) };
  }
  if (type.kind === "enum" || (type.kind === "scalar" && type.scalar !== "float32" && type.scalar !== "float64")) {
    return { kind: "int", value: field.defaultInteger() };
  }
  if (type.kind === "scalar") {
    return { kind: "real", value: field.defaultReal() };
  }
  return { kind: "none" };
};

const readField =
  (schema: Schema) =>
  (field: Field): FbsField => {
    const type = readType(schema, field.type()!);
    return { name: field.name() ?? "", id: field.id(), type, default: readDefault(type, field) };
  };

// A union field reflects as two: its hidden `<name>_type` discriminator
// (`UType`) and the value. The model keeps only the value; the discriminator's
// slot is always the one just before it.
const readTable =
  (schema: Schema) =>
  (obj: Object_): FbsTable => ({
    name: bareName(obj.name() ?? ""),
    fields: times(obj.fieldsLength(), (i) => obj.fields(i)!)
      .filter((f) => f.type()!.baseType() !== BaseType.UType)
      .map(readField(schema))
      .toSorted((a, b) => a.id - b.id),
    ...(hasAttribute(obj, "paginated") ? { paginated: true as const } : {}),
  });

const readEnum = (en: Enum): FbsEnum => {
  const underlying = INTEGER_SCALARS.get(en.underlyingType()!.baseType());
  if (underlying === undefined) {
    throw new Error(`enum ${en.name()} has a non-integer underlying type`);
  }
  const bitFlags = hasAttribute(en, "bit_flags");
  // A 64-bit enum's wire value is a bigint, but a TS numeric enum is backed by
  // number — the two can't reconcile, so reject rather than emit broken code.
  // A `bit_flags` enum is a list of flag names in TS, so any width works.
  if (!bitFlags && (underlying === "int64" || underlying === "uint64")) {
    throw new Error(
      `enum ${en.name()} uses a 64-bit underlying type, which is only supported for a bit_flags enum; ` +
        "if it is declared (bit_flags), rebuild the .bfbs with `flatc -b --schema --bfbs-builtins`",
    );
  }
  if (bitFlags && underlying.startsWith("int")) {
    throw new Error(`bit_flags enum ${en.name()} must have an unsigned underlying type`);
  }
  return {
    name: bareName(en.name() ?? ""),
    underlying,
    members: times(en.valuesLength(), (i) => en.values(i)!).map((v) => ({
      name: v.name() ?? "",
      value: v.value(),
    })),
    ...(bitFlags ? { bitFlags } : {}),
  };
};

const readUnion =
  (schema: Schema) =>
  (en: Enum): FbsUnion => ({
    name: bareName(en.name() ?? ""),
    members: times(en.valuesLength(), (i) => en.values(i)!)
      .filter((v) => v.value() !== 0n)
      .map((v) => {
        const type = v.unionType();
        const obj = type?.baseType() === BaseType.Obj ? schema.objects(type.index()) : null;
        if (obj == null || obj.isStruct()) {
          throw new Error(`union ${en.name()} member ${v.name()} is not a table, which is not supported`);
        }
        return { name: v.name() ?? "", table: bareName(obj.name() ?? ""), value: Number(v.value()) };
      }),
  });

/** Read a served `.bfbs` reflection buffer into the wire-schema model. */
export const readBfbs = (bytes: Uint8Array): FbsSchema => {
  const bb = new flatbuffers.ByteBuffer(bytes);
  if (!Schema.bufferHasIdentifier(bb)) {
    throw new Error("not a FlatBuffer reflection schema: missing 'BFBS' magic identifier");
  }
  const schema = Schema.getRootAsSchema(bb);
  const tables = times(schema.objectsLength(), (i) => schema.objects(i)!)
    .filter((obj) => !obj.isStruct())
    .map(readTable(schema));
  const allEnums = times(schema.enumsLength(), (i) => schema.enums(i)!);
  const enums = allEnums.filter((en) => !en.isUnion()).map(readEnum);
  const unions = allEnums.filter((en) => en.isUnion()).map(readUnion(schema));
  // Tables, enums, and unions share one TS namespace; stripping the FlatBuffer
  // namespace prefix can make two distinct types collide on their bare name.
  const seen = new Set<string>();
  for (const name of [...tables.map((t) => t.name), ...enums.map((e) => e.name), ...unions.map((u) => u.name)]) {
    if (seen.has(name)) {
      throw new Error(`type name '${name}' is declared more than once after stripping namespaces`);
    }
    seen.add(name);
  }
  return { tables, enums, unions };
};
