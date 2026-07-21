# @plonklabs/flatbed-client

The runtime for TypeScript FlatBuffer clients of
[flatbed](https://github.com/plonklabs/flatbed) services.

This package provides the stable runtime a generated client extends: the base
`FlatbedClient` (which owns the wire rules — `application/x-flatbuffers` content
negotiation, the GET/HEAD no-body constraint, baseUrl joining, error mapping),
the `FlatbedError` type, and a swappable `Transport`.

## Transport — open for extension, closed for modification

The client depends on a `Transport` interface, never on `fetch` directly. The
default is `fetch`; provide your own to use axios, add auth headers, retries, or
interceptors — without editing any generated code:

```ts
import { FlatbedClient, type Transport } from "@plonklabs/flatbed-client";

const axiosTransport: Transport = {
  async send({ method, url, headers, body }) {
    const r = await axios.request({
      method, url, headers, data: body,
      responseType: "arraybuffer", validateStatus: () => true,
    });
    return { status: r.status, ok: r.status >= 200 && r.status < 300, body: new Uint8Array(r.data) };
  },
};

const api = new FlatbedClient({ baseUrl: "http://svc:8080", transport: axiosTransport });
```

## Generating a client

`flatbed-client generate` reads a running service's `/openapi.json` (operations)
and `/schema.bfbs` (the FlatBuffer wire schema) and emits a typed client — pure
TypeScript, no Rust toolchain and no `flatc`:

```bash
npm i -D @plonklabs/flatbed-client
npx flatbed-client generate --server http://localhost:8080 --out src/api
```

It writes four files into `--out`:

- `types.ts` — an interface per table and a numeric enum (with true wire values)
  per enum.
- `codec.ts` — per-table `encode…Root` / `decode…Root` over the `flatbuffers`
  runtime, byte-identical to flatbed's Rust codec.
- `client.ts` — a `FlatbedClient` (extending this package's base) with one method
  per route, talking `application/x-flatbuffers`.
- `index.ts` — a barrel re-exporting the folder.

```ts
import { FlatbedClient, Priority } from "./api";

const api = new FlatbedClient({ baseUrl: "http://localhost:8080" });
const res = await api.postEcho({ message: "hi", times: 3, priority: Priority.Low });
// => { message: "hi hi hi" }  — sent and received as application/x-flatbuffers
```

Pass `--openapi <file> --schema <file>` instead of `--server` to generate from
saved files offline.
