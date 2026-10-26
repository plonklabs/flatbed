# @plonklabs/flatbed-client

The runtime and code generator for TypeScript FlatBuffer clients of
[flatbed](https://github.com/plonklabs/flatbed) services.

`flatbed-client generate` reads a running service's `/openapi.json` (operations)
and `/schema.bfbs` (the FlatBuffer wire schema) and emits a typed client — pure
TypeScript, no Rust toolchain and no `flatc`:

```bash
npm i -D @plonklabs/flatbed-client
npx flatbed-client generate --server http://localhost:8080 --out src/api
```

It writes five files into `--out`:

- `types.ts` — an interface per table, a numeric enum (true wire values) per
  enum, a union of flag names per `bit_flags` enum (a field holds the list of
  its set flags), and a `{ type, value }` tagged type per union.
- `codec.ts` — `encode…Root` for each request body and `decode…Root` for each
  response body, over the `flatbuffers` runtime, byte-identical to flatbed's Rust
  codec. Only the directions a client uses are emitted.
- `json-codec.ts` — `encode…Json` for each request body and `decode…Json` for
  each response body, matching flatbed's JSON wire shape (enums as variant names,
  flags as lists of flag names, a union as `<field>_type` beside `<field>`,
  numbers for scalars).
- `client.ts` — a `createFlatbedClient(config)` factory with one method per route.
- `index.ts` — a barrel re-exporting the folder.

Pass `--openapi <file> --schema <file>` instead of `--server` to generate from
saved files offline. A `.bfbs` you build yourself needs `flatc -b --schema
--bfbs-builtins`: without `--bfbs-builtins`, flatc drops the `bit_flags`
attribute and flag enums generate as plain enums (or fail, when 64-bit).

## Using the client

```ts
import { createFlatbedClient, Priority } from "./api";

const api = createFlatbedClient({ baseUrl: "http://localhost:8080" });

// Each method takes a single object; `body` is the operation's Request Table,
// `pathParams` its path parameters — only the keys the operation actually has.
const echo = await api.postEcho({ body: { message: "hi", times: 3, priority: Priority.Low } });
const user = await api.getUsersById({ pathParams: { id: "42" } });
const saved = await api.putUsersById({ body: { name: "x" }, pathParams: { id: "42" } });
```

`ClientConfig` also takes headers added to every request:

```ts
const api = createFlatbedClient({
  baseUrl: "http://svc:8080",
  headers: { authorization: `Bearer ${token}` },
});
```

## Paginated lists

An operation whose request and response are `(paginated)` tables becomes a list
call: `body` takes the filters and `page_size` (never `page`), and it resolves to
a `Page`:

```ts
const first = await api.listItems({ body: { page_size: 20, owner: "ann", status: ItemStatus.Open, order: ItemOrder.Newest } });
first.items;        // the response's item vector
first.applied;      // the request the server applied, when the response echoes it
first.nextPage;     // raw cursors, e.g. for links; undefined at the ends
first.previousPage;
const second = await first.next?.(); // undefined on the last page
```

`next()`/`previous()` send only the cursor and the same `page_size` — a cursor
already carries the filters it was minted with, and the server rejects a cursor
sent alongside a filter. To change filters, call the list again without a cursor.

## Choosing the wire format

Every call sends FlatBuffer by default. Pass `{ as: "json" }` as a second
argument to make that call over JSON instead — the option is generated only for
operations the spec advertises for both formats:

```ts
// FlatBuffer (default)
const echo = await api.postEcho({ body: { message: "hi", times: 3, priority: Priority.Low } });

// same operation, JSON — chosen per call
const echoJson = await api.postEcho({ body: { message: "hi", times: 3, priority: Priority.Low } }, { as: "json" });
```

FlatBuffer is compact and covers the full wire range; JSON is human-readable.
**On the JSON path, 64-bit integers (`bigint`) are limited to the safe-integer
range** — the server encodes them as JSON numbers, so a value above 2^53 loses
precision. Use the FlatBuffer path when full 64-bit range matters.

## Transport — open for extension, closed for modification

The client depends on a `Transport` interface, never on `fetch` directly. The
default is `fetch`; provide your own to use axios, add retries, or interceptors —
without editing any generated code:

```ts
import { createFlatbedClient, type Transport } from "@plonklabs/flatbed-client";

const axiosTransport: Transport = {
  send: ({ method, url, headers, body }) =>
    axios
      .request({ method, url, headers, data: body, responseType: "arraybuffer", validateStatus: () => true })
      .then((r) => ({
        status: r.status,
        ok: r.status >= 200 && r.status < 300,
        headers: r.headers as Record<string, string>,
        body: new Uint8Array(r.data),
      })),
};

const api = createFlatbedClient({ baseUrl: "http://svc:8080", transport: axiosTransport });
```

`headers` (lowercase names) is optional, but without it a `FlatbedError` loses
the server's error identity below.

## Errors

A non-2xx response rejects with `FlatbedError`:

| Field | Source |
| --- | --- |
| `status` | the HTTP status |
| `code` | `x-error-code`, or the body's `code` when the response is JSON |
| `serverMessage` | `x-error-message`, or the body's `message` when the response is JSON |
| `requestId` | `x-request-id` |

Each identity field is `undefined` when the response doesn't carry it: a 404
for an unknown path, a proxy-generated error, a transport without headers. A
framework-level failure (a request body that doesn't decode, say) carries `code`
and `serverMessage` but no `requestId`.

Across origins the browser hides these headers unless the service's CORS
response lists them in `Access-Control-Expose-Headers`.

`requestId` is what links a frontend failure to the backend's: a flatbed
service built with the `sentry` feature tags its events `request_id`, so tag
the frontend event the same way with your app's own `@sentry/browser`
(the client has no Sentry dependency):

```ts
import * as Sentry from "@sentry/browser";
import { FlatbedError } from "@plonklabs/flatbed-client";

api.getUsersById({ pathParams: { id: "42" } }).catch((e: unknown) => {
  if (e instanceof FlatbedError) {
    Sentry.captureException(e, { tags: { request_id: e.requestId, error_code: e.code } });
  }
  throw e;
});
```
