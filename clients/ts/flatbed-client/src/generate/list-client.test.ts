import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

import type { Page } from "../page.js";
import type { FlatbedRequest, FlatbedResponse, Transport } from "../transport.js";
import { emitClient } from "./emit-client.js";
import { emitCodec } from "./emit-codec.js";
import { emitJson } from "./emit-json.js";
import { emitTypes } from "./emit-types.js";
import type { CodecRoots, Operation } from "./model.js";
import { readBfbs } from "./read-bfbs.js";

const schema = readBfbs(readFileSync(fileURLToPath(new URL("./__fixtures__/test.bfbs", import.meta.url))));
const runtime = fileURLToPath(new URL("../index.js", import.meta.url));
const dir = fileURLToPath(new URL("../../node_modules/.cache/flatbed-list-test", import.meta.url));

const ops: readonly Operation[] = [
  { method: "POST", path: "/items", operationId: "listItems", requestType: "ListItemsRequest", responseType: "ListItemsResponse", supportsJson: true },
];
const client: CodecRoots = { encodeRoots: new Set(["ListItemsRequest"]), decodeRoots: new Set(["ListItemsResponse"]) };
const server: CodecRoots = { encodeRoots: client.decodeRoots, decodeRoots: client.encodeRoots };

interface Item {
  readonly id?: string;
}
interface Req {
  readonly page?: string;
  readonly page_size: number;
  readonly owner?: string;
  readonly status: number;
  readonly order: number;
}
type Codec = (bytes: Uint8Array) => unknown;
type Encoder = (value: unknown) => Uint8Array;
interface Generated {
  readonly createFlatbedClient: (config: { baseUrl: string; transport: Transport }) => {
    listItems: (args: { body: Omit<Req, "page"> }, opts?: { as?: "json" | "flatbuffer" }) => Promise<Page<Item, Req>>;
  };
}

// The generated client imports the published package; point it at this source tree instead.
const generated: Promise<{ client: Generated; server: Record<string, Codec | Encoder> }> = (() => {
  mkdirSync(dir, { recursive: true });
  writeFileSync(`${dir}/types.ts`, emitTypes(schema));
  writeFileSync(`${dir}/codec.ts`, emitCodec(schema, client));
  writeFileSync(`${dir}/json-codec.ts`, emitJson(schema, client));
  writeFileSync(`${dir}/server-codec.ts`, emitCodec(schema, server));
  writeFileSync(`${dir}/server-json-codec.ts`, emitJson(schema, server));
  writeFileSync(`${dir}/client.ts`, emitClient(ops, schema).replace('"@plonklabs/flatbed-client"', JSON.stringify(runtime)));
  return Promise.all([
    import(`${dir}/client.ts`) as Promise<Generated>,
    import(`${dir}/server-codec.ts`) as Promise<Record<string, Codec | Encoder>>,
    import(`${dir}/server-json-codec.ts`) as Promise<Record<string, Codec | Encoder>>,
  ]).then(([c, fb, json]) => ({ client: c, server: { ...fb, ...json } }));
})();

const ITEMS = ["i0", "i1", "i2", "i3", "i4"].map((id, n) => ({ id, owner: "ann", created_at: BigInt(n), status: 1 }));

/**
 * A list endpoint over `ITEMS` with `c<offset>` cursors. A cursor request
 * echoes the first request's filters as `applied`, as a real cursor carries
 * them. An absent next cursor is sent as "" and an absent previous one omitted.
 */
const listServer = (codecs: Record<string, Codec | Encoder>) => {
  const sent: Req[] = [];
  const transport: Transport = {
    send: (req: FlatbedRequest): Promise<FlatbedResponse> => {
      const json = req.headers["content-type"] === "application/json";
      const decode = codecs[json ? "decodeListItemsRequestJson" : "decodeListItemsRequestRoot"] as Codec;
      const encode = codecs[json ? "encodeListItemsResponseJson" : "encodeListItemsResponseRoot"] as Encoder;
      const body = decode(req.body!) as Req;
      sent.push(body);
      const size = body.page_size;
      const offset = body.page === undefined ? 0 : Number(body.page.slice(1));
      const response = {
        items: ITEMS.slice(offset, offset + size),
        page: {
          next_page: offset + size < ITEMS.length ? `c${offset + size}` : "",
          previous_page: offset > 0 ? `c${Math.max(0, offset - size)}` : undefined,
        },
        applied: { ...sent[0], page: undefined },
      };
      return Promise.resolve({ status: 200, ok: true, body: encode(response) });
    },
  };
  return { transport, sent };
};

const ids = (page: Page<Item, Req>): readonly (string | undefined)[] => page.items.map((i) => i.id);

const CURSOR_BODY = { page_size: 2, owner: undefined, status: 0, order: 0 };

(["flatbuffer", "json"] as const).forEach((as) => {
  test(`${as}: next/previous walk the list sending only page and page_size, ending in undefined`, () =>
    generated.then(({ client: gen, server: codecs }) => {
      const { transport, sent } = listServer(codecs);
      const api = gen.createFlatbedClient({ baseUrl: "http://svc", transport });
      return api
        .listItems({ body: { page_size: 2, owner: "ann", status: 1, order: 1 } }, { as })
        .then((p1) => {
          assert.deepEqual(ids(p1), ["i0", "i1"]);
          assert.equal(p1.previous, undefined);
          assert.equal(p1.previousPage, undefined);
          assert.equal(p1.nextPage, "c2");
          assert.deepEqual(sent[0], { page: undefined, page_size: 2, owner: "ann", status: 1, order: 1 });
          return p1.next!();
        })
        .then((p2) => {
          assert.deepEqual(ids(p2), ["i2", "i3"]);
          assert.deepEqual(sent[1], { ...CURSOR_BODY, page: "c2" });
          assert.equal(p2.previousPage, "c0");
          return p2.next!();
        })
        .then((p3) => {
          assert.deepEqual(ids(p3), ["i4"]);
          assert.deepEqual(sent[2], { ...CURSOR_BODY, page: "c4" });
          assert.equal(p3.next, undefined);
          assert.equal(p3.nextPage, undefined);
          return p3.previous!();
        })
        .then((back) => {
          assert.deepEqual(ids(back), ["i2", "i3"]);
          assert.deepEqual(sent[3], { ...CURSOR_BODY, page: "c2" });
        });
    }));

  test(`${as}: applied surfaces the request the page was made from`, () =>
    generated.then(({ client: gen, server: codecs }) => {
      const api = gen.createFlatbedClient({ baseUrl: "http://svc", transport: listServer(codecs).transport });
      return api
        .listItems({ body: { page_size: 2, owner: "ann", status: 1, order: 1 } }, { as })
        .then((p1) => {
          assert.deepEqual(p1.applied, { page: undefined, page_size: 2, owner: "ann", status: 1, order: 1 });
          return p1.next!();
        })
        .then((p2) => assert.equal(p2.applied?.owner, "ann"));
    }));
});

test("the generated list client type-checks, and refuses page on the first call", () =>
  generated.then(() => {
    writeFileSync(
      `${dir}/consumer.ts`,
      `import type { Page } from ${JSON.stringify(runtime)};\n` +
        'import { createFlatbedClient } from "./client.js";\n' +
        'import { ItemOrder, ItemStatus, type ListItemsRequest, type ListedItem } from "./types.js";\n' +
        'const api = createFlatbedClient({ baseUrl: "http://svc" });\n' +
        "export const first: Promise<Page<ListedItem, ListItemsRequest>> =\n" +
        "  api.listItems({ body: { page_size: 2, status: ItemStatus.Open, order: ItemOrder.Oldest } });\n" +
        "// @ts-expect-error a first call carries filters, never a cursor\n" +
        'api.listItems({ body: { page: "c2", page_size: 2, status: ItemStatus.Any, order: ItemOrder.Newest } });\n',
    );
    writeFileSync(
      `${dir}/tsconfig.json`,
      JSON.stringify({
        compilerOptions: {
          target: "es2022",
          module: "nodenext",
          moduleResolution: "nodenext",
          lib: ["es2023", "dom"],
          strict: true,
          noUnusedLocals: true,
          noUnusedParameters: true,
          noEmit: true,
          skipLibCheck: true,
          verbatimModuleSyntax: true,
        },
        files: ["client.ts", "consumer.ts"],
      }),
    );
    const tsc = createRequire(import.meta.url).resolve("typescript/bin/tsc");
    execFileSync(process.execPath, [tsc, "-p", dir], { stdio: "pipe" });
  }));
