#!/usr/bin/env node
import { fetchInput, generateFiles, readInput, writeFiles, type GenerateInput } from "./generate/generate.js";

const USAGE =
  "usage: flatbed-client generate (--server <url> | --openapi <file> --schema <file>) --out <dir>";

const flag = (name: string): string | undefined => {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 ? process.argv[i + 1] : undefined;
};

/** Resolve the generator input from either a live server or local files. */
const inputFrom = (): Promise<GenerateInput> => {
  const server = flag("server");
  const openapi = flag("openapi");
  const schema = flag("schema");
  if (server !== undefined) return fetchInput(server);
  if (openapi !== undefined && schema !== undefined) return readInput(openapi, schema);
  return Promise.reject(new Error(USAGE));
};

const run = (): Promise<void> => {
  const out = flag("out");
  if (process.argv[2] !== "generate" || out === undefined) return Promise.reject(new Error(USAGE));
  return inputFrom()
    .then(generateFiles)
    .then((files) => writeFiles(out, files))
    .then(() => console.log(`flatbed-client: wrote types.ts, codec.ts, client.ts, index.ts to ${out}`));
};

run().catch((error: unknown) => {
  console.error(`flatbed-client: ${error instanceof Error ? error.message : String(error)}`);
  process.exit(1);
});
