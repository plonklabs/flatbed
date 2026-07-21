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

It writes four files into `--out`:

- `types.ts` — an interface per table and a numeric enum (true wire values).
- `codec.ts` — per-table `encode…Root` / `decode…Root` over the `flatbuffers`
  runtime, byte-identical to flatbed's Rust codec.
- `client.ts` — a `createFlatbedClient(config)` factory with one method per route.
- `index.ts` — a barrel re-exporting the folder.

Pass `--openapi <file> --schema <file>` instead of `--server` to generate from
saved files offline.

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
      .then((r) => ({ status: r.status, ok: r.status >= 200 && r.status < 300, body: new Uint8Array(r.data) })),
};

const api = createFlatbedClient({ baseUrl: "http://svc:8080", transport: axiosTransport });
```

A non-2xx response rejects with `FlatbedError` carrying the `status`.
