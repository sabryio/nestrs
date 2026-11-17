# react-orpc

A React app that talks to the nestrs `blog-api` through a **typed oRPC client**, with
no oRPC server anywhere: the contract is *generated from the Rust server's OpenAPI
document*, and `OpenAPILink` turns every call into a plain REST request against it.
The Rust handlers are the only place the API is written down.

| Piece | What it does |
|---|---|
| `openapi.json` | The Rust server's OpenAPI document (`blog-api --print-openapi`), committed so API changes show up in review. |
| `scripts/generate-contract.ts` | Generates `src/contract.generated.ts` from it. |
| `src/contract.generated.ts` | **Generated, never edited.** zod schemas + the oRPC contract (`oc.route(..).input(..).output(..)`). |
| `src/client.ts` | `OpenAPILink` + `createORPCClient`, bearer token injection, TanStack Query utils. |
| `src/errors.ts` | Decodes the Rust `{"error":{code,message,details}}` envelope into `ORPCError`. |
| `src/App.tsx` | `useQuery`/`useMutation` for REST; `streamedOptions` for the live feed. |
| `scripts/generate-contract.test.ts` | Unit tests for the generator. |
| `e2e/` | vitest against the **real Rust binary**: CRUD, errors, typed SSE, SIGTERM shutdown. |

## The contract is generated

```
Rust handlers + DTOs ──► OpenAPI (openapi.json) ──► contract.generated.ts ──► typed client
```

```sh
npm run gen             # nestrs generate contract: read the Rust controllers/DTOs with syn, write the contract
npm run check:contract  # CI: fail (with a diff) if the contract is stale or anything had to be left out
npm run gen:openapi     # alternative: go through the running server's OpenAPI document instead
```

What the generator does with the spec:

- one zod schema and TypeScript type per component schema, dependencies first;
- operations grouped by controller: `PostsController_find` → `contract.posts.find`;
- a compact input: path params, query params and the JSON body's properties in one
  object; the output of the lowest 2xx response (`z.void()` for 204);
- a tagged `oneOf` becomes `z.discriminatedUnion`, and a `text/event-stream` response
  becomes `eventIterator(<message schema>)`, which is how the event type reaches the UI;
- 4xx/5xx responses become `.errors({...})`: the status picks oRPC's code (404 →
  `NOT_FOUND`) and the error envelope's body becomes the error's typed `data`;
- the shared path prefix (`/api`) is exported as `apiPrefix` for the link.

It handles the OpenAPI subset the nestrs generator emits and **refuses** anything else
(`allOf`, recursive schemas, header parameters, non-JSON bodies) with a message naming
the operation, instead of guessing `z.any()`.

Validation rules only reach the client if the spec states them, so the Rust DTOs carry
`#[schema(min_length = 1, max_length = 200)]` next to their `#[validate(..)]`. The server
still validates everything; the generated rules just give the client the same limits.

I tried Hey API's `orpc` plugin first (the route oRPC documents). It emitted flat,
input-only procedures with no outputs and nothing for the SSE route, which is exactly
what this example needs, hence the small purpose-built generator.

## Typed errors

The `#[response(404, "..")]` attributes on the Rust handlers are the declaration; the
generated contract carries them as `.errors(..)`, so a call's failures are part of its type:

```ts
const [error, post] = await safe(client.posts.find({ id }));
if (isDefinedError(error)) {
  error.code;       // "NOT_FOUND"  (exactly the codes `posts.find` declares)
  error.data.code;  // "not_found"  (the server's stable machine code)
  error.data;       // ErrorBody: { code, message, details? }, generated from the spec
}
```

`src/errors.ts` has the two pieces that make this work with a non-oRPC server: a
decoder from the Rust envelope to `ORPCError` (code from the HTTP status), and a link
interceptor that flags the error as *defined* only when that procedure's contract
declares the code. A failure the contract does not declare (say a 400 from a route that
lists none) is still thrown as a plain `ORPCError`, just not part of the typed surface.

## Typed Server-Sent Events

`GET /posts/events` returns `EventStream<PostEventDto>` in Rust and
`eventIterator(PostEvent)` in the contract. On the wire, every stream is:

```
: \n\n                      flush (first bytes, also the 15 s keep-alive)
data: {"type":"published","post":{..}}\n\n     one per message
event: close\n\n            terminal frame, then the connection ends
```

Failures end with `event: error` instead of `close`. On shutdown the server sends
`close` to every open stream, so graceful shutdown never hangs. The client side is an
`AsyncIterator<PostEvent>`; `switch (event.type)` is exhaustive.

## Reconnecting without losing events

Messages carry an id (`id: <epoch>-<seq>`). When the connection drops, oRPC's
`RetryLinkPlugin` reconnects and sends the last id it delivered as `Last-Event-ID`.
The server replays what was missed, then continues live, and the `for await` loop in
the UI never notices:

```
client ◄── id: 7-1 / id: 7-2 ──  server
   ✂ connection drops;  server publishes 7-3, 7-4
client ── Last-Event-ID: 7-2 ──►  server   replays 7-3, 7-4, then live
```

- `epoch` is one number per server start, so an id from before a restart is never
  taken for a position in the new run: it starts a live stream instead.
- History is bounded (256 events). Replay is best effort, so after a reconnect the
  client refetches the post list (`onReconnect` in `src/client.ts`).
- A dropped connection or missing `close` is retried; an `event: error` frame is the
  server giving up and is not. `close` is completion and is never retried.
- `e2e/proxy.ts` is a TCP proxy the tests cut mid-stream to prove it: events published
  during the outage arrive exactly once, in order.

## Run it

```sh
cargo run -p blog-api          # Rust API on :3000 (in-memory, token "dev-token")
cd examples/react-orpc
npm install
npm run dev                    # http://localhost:5173, /api proxied to :3000
```

```sh
npm run typecheck && npm run check:spec && npm test && npm run build
```
