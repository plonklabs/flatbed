/**
 * The wire-schema model the generators consume — a small, closed vocabulary
 * read from a served `.bfbs`. Each generator (types, codec, client) is a pure
 * function of this model.
 */

/** A FlatBuffer scalar, named by its wire width. */
export type ScalarType =
  | "bool"
  | "int8"
  | "uint8"
  | "int16"
  | "uint16"
  | "int32"
  | "uint32"
  | "int64"
  | "uint64"
  | "float32"
  | "float64";

/** A field's wire type, as a discriminated union. */
export type FbsType =
  | { readonly kind: "scalar"; readonly scalar: ScalarType }
  | { readonly kind: "string" }
  | { readonly kind: "enum"; readonly name: string }
  | { readonly kind: "table"; readonly name: string }
  | { readonly kind: "vector"; readonly element: FbsType };

/** A scalar/enum field's declared default, used for omit-if-equal + decode. */
export type FbsDefault =
  | { readonly kind: "none" }
  | { readonly kind: "int"; readonly value: bigint }
  | { readonly kind: "real"; readonly value: number };

/** One table field. `id` is the vtable slot; fields are sorted by it. */
export interface FbsField {
  readonly name: string;
  readonly id: number;
  readonly type: FbsType;
  readonly default: FbsDefault;
}

export interface FbsTable {
  readonly name: string;
  readonly fields: readonly FbsField[];
}

export interface FbsEnumMember {
  readonly name: string;
  readonly value: bigint;
}

export interface FbsEnum {
  readonly name: string;
  readonly underlying: ScalarType;
  readonly members: readonly FbsEnumMember[];
}

/** The full wire schema read from a `.bfbs`. */
export interface FbsSchema {
  readonly tables: readonly FbsTable[];
  readonly enums: readonly FbsEnum[];
}

/**
 * An operation that speaks `application/x-flatbuffers`, read from the OpenAPI
 * spec. `requestType`/`responseType` are the bare `$ref` component names, absent
 * when that body is missing or inlined rather than referenced.
 */
export interface Operation {
  readonly method: string;
  readonly path: string;
  readonly operationId?: string;
  readonly requestType?: string;
  readonly responseType?: string;
}
