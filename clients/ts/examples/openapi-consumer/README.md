# flatbed-client example: openapi-consumer

A runnable example of a generated [`@plonklabs/flatbed-client`](../../flatbed-client)
talking to the [`examples/openapi`](../../../../examples/openapi) flatbed service.

## What it shows

`src/main.ts` creates a client and calls the service's two operations over **both**
wire formats — FlatBuffer (the default) and JSON, selected per call:

```ts
const client = createFlatbedClient({ baseUrl: "http://localhost:8080" });

// FlatBuffer (default)
const greeting = await client.postGreet({ body: { name: "Ada" } });

// same operation, JSON — chosen per call
const greetingJson = await client.postGreet({ body: { name: "Ada" } }, { as: "json" });

// a richer request: enum + uint32 + vector of tables
await client.postEcho({
  body: { message: "hi", times: 3, priority: Priority.High, tags: [{ label: "demo" }] },
});
```

## Generated code

`src/generated/` is the committed output of
`flatbed-client generate --server http://localhost:8080`, run against
`examples/openapi`. It's checked in so the API surface is browsable, and CI
regenerates and diffs it (`scripts/verify-fb-client-npm.sh`) so it can't drift
from the service's schema.

## Running it

```bash
# 1. boot the service (needs flatc on PATH)
cargo run -p flatbed-example-openapi   # serves on :8080

# 2. from the repo root, build and run the example
npm ci
npm run -w @plonklabs/flatbed-openapi-example build
npm run -w @plonklabs/flatbed-openapi-example start
```

`scripts/verify-fb-client-npm.sh` does all of this end-to-end (boot → regenerate
+ diff → build → run) with no Rust or flatc in the consumer loop.
