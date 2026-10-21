/**
 * The wire-schema model the generators consume — a small, closed vocabulary
 * read from a served `.bfbs`. Each generator is a pure function of this model.
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

/**
 * A field's wire type, as a discriminated union. `flags` is a `bit_flags` enum
 * (a value is any OR of its flags); `union` is a FlatBuffer union, whose
 * discriminator occupies the slot just before the field's own id.
 */
export type FbsType =
  | { readonly kind: "scalar"; readonly scalar: ScalarType }
  | { readonly kind: "string" }
  | { readonly kind: "enum"; readonly name: string }
  | { readonly kind: "flags"; readonly name: string }
  | { readonly kind: "union"; readonly name: string }
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

/**
 * A FlatBuffer table, its fields ordered by wire id. `paginated` is set when
 * the table carries the schema's `(paginated)` attribute.
 */
export interface FbsTable {
  readonly name: string;
  readonly fields: readonly FbsField[];
  readonly paginated?: true;
}

/** One enum variant and its wire value. */
export interface FbsEnumMember {
  readonly name: string;
  readonly value: bigint;
}

/**
 * A FlatBuffer enum: its backing integer scalar and its variants. A `bitFlags`
 * enum's member values are the flags' masks.
 */
export interface FbsEnum {
  readonly name: string;
  readonly underlying: ScalarType;
  readonly members: readonly FbsEnumMember[];
  readonly bitFlags?: boolean;
}

/** One union variant: its name, the table it carries, and its discriminator. */
export interface FbsUnionMember {
  readonly name: string;
  readonly table: string;
  readonly value: number;
}

/** A FlatBuffer union of tables (the implicit `NONE` is left out). */
export interface FbsUnion {
  readonly name: string;
  readonly members: readonly FbsUnionMember[];
}

/** The full wire schema read from a `.bfbs`. */
export interface FbsSchema {
  readonly tables: readonly FbsTable[];
  readonly enums: readonly FbsEnum[];
  readonly unions?: readonly FbsUnion[];
}

/**
 * An operation that speaks `application/x-flatbuffers`, read from the OpenAPI
 * spec. `requestType`/`responseType` are the bare `$ref` component names, absent
 * when that body is missing or inlined rather than referenced. `supportsJson` is
 * true when every direction the operation uses also advertises
 * `application/json`, so the call can be made with `{ as: "json" }`.
 */
export interface Operation {
  readonly method: string;
  readonly path: string;
  readonly operationId?: string;
  readonly requestType?: string;
  readonly responseType?: string;
  readonly supportsJson: boolean;
}

/**
 * The body types a client actually uses, so the codec emitters can skip the
 * dead direction of every type. A client encodes request bodies (`encodeRoots`)
 * and decodes response bodies (`decodeRoots`); it never decodes a request or
 * encodes a response — those halves are the server's.
 */
export interface CodecRoots {
  readonly encodeRoots: ReadonlySet<string>;
  readonly decodeRoots: ReadonlySet<string>;
}
