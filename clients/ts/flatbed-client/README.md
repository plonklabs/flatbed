# @plonklabs/flatbed-client

Runtime and (soon) code generator for TypeScript FlatBuffer clients of
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
    return { status: r.status, ok: r.status < 400, body: new Uint8Array(r.data) };
  },
};

const api = new FlatbedClient({ baseUrl: "http://svc:8080", transport: axiosTransport });
```

The code generator (which emits `types.ts` / `codec.ts` / `client.ts` /
`index.ts` from a live server's `/openapi.json` + `/schema.bfbs`) lands in a
later change.
