import { mkdir, readFile, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { checkNames, checkTypes, emitClient, emitIndex } from "./emit-client.js";
import { emitCodec } from "./emit-codec.js";
import { emitTypes } from "./emit-types.js";
import { readBfbs } from "./read-bfbs.js";
import { readOperations } from "./read-openapi.js";

/** The two inputs the generator reads: the OpenAPI spec and the `.bfbs`. */
export interface GenerateInput {
  readonly spec: unknown;
  readonly bfbs: Uint8Array;
}

/** The four files the generator emits, keyed by filename. */
export interface GeneratedFiles {
  readonly "types.ts": string;
  readonly "codec.ts": string;
  readonly "client.ts": string;
  readonly "index.ts": string;
}

/** Pure generation: an OpenAPI spec + `.bfbs` reflection → the four sources. */
export const generateFiles = ({ spec, bfbs }: GenerateInput): GeneratedFiles => {
  const schema = readBfbs(bfbs);
  const ops = readOperations(spec);
  checkNames(ops);
  checkTypes(schema, ops);
  return {
    "types.ts": emitTypes(schema),
    "codec.ts": emitCodec(schema),
    "client.ts": emitClient(ops),
    "index.ts": emitIndex(),
  };
};

const ensureOk =
  (what: string) =>
  (r: Response): Response => {
    if (!r.ok) throw new Error(`fetching ${what} failed: ${r.status} ${r.statusText}`);
    return r;
  };

/** Fetch `/openapi.json` + `/schema.bfbs` from a running flatbed server. */
export const fetchInput = (
  server: string,
  fetchImpl: typeof globalThis.fetch = globalThis.fetch,
): Promise<GenerateInput> => {
  const base = server.replace(/\/+$/, "");
  return Promise.all([
    fetchImpl(`${base}/openapi.json`)
      .then(ensureOk("/openapi.json"))
      .then((r) => r.json() as Promise<unknown>),
    fetchImpl(`${base}/schema.bfbs`)
      .then(ensureOk("/schema.bfbs"))
      .then((r) => r.arrayBuffer())
      .then((b) => new Uint8Array(b)),
  ]).then(([spec, bfbs]) => ({ spec, bfbs }));
};

/** Read `/openapi.json` + `/schema.bfbs` from files instead of a server. */
export const readInput = (openapiPath: string, schemaPath: string): Promise<GenerateInput> =>
  Promise.all([
    readFile(openapiPath, "utf8").then((s) => JSON.parse(s) as unknown),
    readFile(schemaPath).then((b) => new Uint8Array(b)),
  ]).then(([spec, bfbs]) => ({ spec, bfbs }));

/** Write the generated files into `outDir` (created if missing). */
export const writeFiles = (outDir: string, files: GeneratedFiles): Promise<void> =>
  mkdir(outDir, { recursive: true })
    .then(() =>
      Promise.all(Object.entries(files).map(([name, content]) => writeFile(join(outDir, name), content))),
    )
    .then(() => undefined);
