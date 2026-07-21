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

// An integer base type with `index >= 0` references an enum; `Obj` references a
// table. Everything else is a plain scalar or string. (Unions/structs/arrays
// aren't part of flatbed's supported subset.)
const scalarOrObj = (schema: Schema, base: BaseType, index: number): FbsType => {
  const enumName = index >= 0 && INTEGER_SCALARS.has(base) ? schema.enums(index)?.name() : undefined;
  if (enumName != null) {
    return { kind: "enum", name: bareName(enumName) };
  }
  const scalar = SCALARS.get(base);
  if (scalar !== undefined) {
    return { kind: "scalar", scalar };
  }
  if (base === BaseType.String) {
    return { kind: "string" };
  }
  if (base === BaseType.Obj) {
    return { kind: "table", name: bareName(schema.objects(index)?.name() ?? "") };
  }
  throw new Error(
    `unsupported FlatBuffer base type ${BaseType[base] ?? base}; unions, structs, and arrays are not supported`,
  );
};

const readType = (schema: Schema, type: Type): FbsType =>
  type.baseType() === BaseType.Vector
    ? { kind: "vector", element: scalarOrObj(schema, type.element(), type.index()) }
    : scalarOrObj(schema, type.baseType(), type.index());

const readDefault = (type: FbsType, field: Field): FbsDefault => {
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

const readTable =
  (schema: Schema) =>
  (obj: Object_): FbsTable => ({
    name: bareName(obj.name() ?? ""),
    fields: times(obj.fieldsLength(), (i) => obj.fields(i)!)
      .map(readField(schema))
      .toSorted((a, b) => a.id - b.id),
  });

const readEnum = (en: Enum): FbsEnum => {
  const underlying = INTEGER_SCALARS.get(en.underlyingType()!.baseType());
  if (underlying === undefined) {
    throw new Error(`enum ${en.name()} has a non-integer underlying type`);
  }
  // A 64-bit enum's wire value is a bigint, but a TS numeric enum is backed by
  // number — the two can't reconcile, so reject rather than emit broken code.
  if (underlying === "int64" || underlying === "uint64") {
    throw new Error(`enum ${en.name()} uses a 64-bit underlying type, which is not supported`);
  }
  return {
    name: bareName(en.name() ?? ""),
    underlying,
    members: times(en.valuesLength(), (i) => en.values(i)!).map((v) => ({
      name: v.name() ?? "",
      value: v.value(),
    })),
  };
};

/** Read a served `.bfbs` reflection buffer into the wire-schema model. */
export const readBfbs = (bytes: Uint8Array): FbsSchema => {
  const schema = Schema.getRootAsSchema(new flatbuffers.ByteBuffer(bytes));
  return {
    tables: times(schema.objectsLength(), (i) => schema.objects(i)!)
      .filter((obj) => !obj.isStruct())
      .map(readTable(schema)),
    enums: times(schema.enumsLength(), (i) => schema.enums(i)!).map(readEnum),
  };
};
