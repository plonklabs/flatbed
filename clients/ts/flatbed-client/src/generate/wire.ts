import type { FbsEnum, FbsSchema, ScalarType } from "./model.js";

/** The `flatbuffers` runtime method names + element size for one scalar. */
export interface ScalarOps {
  readonly addField: string;
  readonly vecAdd: string;
  readonly read: string;
  readonly size: number;
  /** The wire zero as a TS literal (`0n` for 64-bit ints, `0` otherwise). */
  readonly zero: string;
  readonly ts: "number" | "bigint" | "boolean";
}

/** `bool` is stored as `int8`; the boolean is converted at the call site. */
export const SCALAR_OPS: Readonly<Record<ScalarType, ScalarOps>> = {
  bool: { addField: "addFieldInt8", vecAdd: "addInt8", read: "readInt8", size: 1, zero: "0", ts: "boolean" },
  int8: { addField: "addFieldInt8", vecAdd: "addInt8", read: "readInt8", size: 1, zero: "0", ts: "number" },
  uint8: { addField: "addFieldInt8", vecAdd: "addInt8", read: "readUint8", size: 1, zero: "0", ts: "number" },
  int16: { addField: "addFieldInt16", vecAdd: "addInt16", read: "readInt16", size: 2, zero: "0", ts: "number" },
  uint16: { addField: "addFieldInt16", vecAdd: "addInt16", read: "readUint16", size: 2, zero: "0", ts: "number" },
  int32: { addField: "addFieldInt32", vecAdd: "addInt32", read: "readInt32", size: 4, zero: "0", ts: "number" },
  uint32: { addField: "addFieldInt32", vecAdd: "addInt32", read: "readUint32", size: 4, zero: "0", ts: "number" },
  int64: { addField: "addFieldInt64", vecAdd: "addInt64", read: "readInt64", size: 8, zero: "0n", ts: "bigint" },
  uint64: { addField: "addFieldInt64", vecAdd: "addInt64", read: "readUint64", size: 8, zero: "0n", ts: "bigint" },
  float32: { addField: "addFieldFloat32", vecAdd: "addFloat32", read: "readFloat32", size: 4, zero: "0", ts: "number" },
  float64: { addField: "addFieldFloat64", vecAdd: "addFloat64", read: "readFloat64", size: 8, zero: "0", ts: "number" },
};

/** enum name → its backing integer scalar, resolved once from the schema. */
export const enumUnderlying = (schema: FbsSchema): ReadonlyMap<string, ScalarType> =>
  new Map(schema.enums.map((e) => [e.name, e.underlying]));

/** The scalar a field's wire slot uses — an enum resolves to its underlying. */
export const wireScalar = (underlying: ReadonlyMap<string, ScalarType>) => (name: string): ScalarType =>
  underlying.get(name) ?? (name as ScalarType);

/** Whether an enum is backed by a 64-bit integer (its literals need an `n`). */
export const isEnum64 = (e: FbsEnum): boolean => e.underlying === "int64" || e.underlying === "uint64";
