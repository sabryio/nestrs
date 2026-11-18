# 9. Ship it

## The client contract, without writing it twice

A React client needs the API's shape in TypeScript. Writing it by hand duplicates what the Rust controllers already say, and
drifts. Generate it:

```text
purlin generate contract --also ../../crates/purlin-health/src --out ../react-orpc/src/contract.generated.ts
```

The command reads the controllers and DTOs with `syn` (no build, no running server) and writes an oRPC contract with zod
schemas: paths and methods from `#[get("/{id}")]`, inputs from `Path`/`Query`/`ValidatedJson`, outputs from `Json<T>` or
`EventStream<T>`, typed errors from `#[response(..)]`, guards and roles, field rules from `#[validate(..)]`, names from serde
attributes. What JSON cannot express is reported and left out rather than guessed. See
`examples/react-orpc` (`npm run gen`, `npm run check:contract`).

## CI gates

Everything above is checkable by a command with a meaningful exit status, so a pipeline can hold the line:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
purlin doctor                                     # layer rules
purlin generate contract --check --strict ...     # the client contract is not stale
```

`purlin doctor` fails when a layer reaches where it must not; `--check` prints a diff and fails when the contract no longer
matches the controllers; `--strict` fails if anything had to be left out.

## Configuration for production

Everything is an environment variable (`BLOG_*`): `BLOG_STORAGE=database`, `BLOG_DATABASE_URL`, `BLOG_MIGRATE_ON_START=false`
when migrations are a deploy step, `BLOG_REDIS_URL` for state shared across instances, `BLOG_OTLP_ENDPOINT` for traces.
The service exposes `/health/live`, `/health/ready` and Prometheus metrics, and shuts down gracefully (in-flight requests,
the outbox relay, the pool).

## Where next

* Add a feature with `purlin generate feature`, then replace its in-memory adapter with a database one behind the same port.
* Add a use case, a controller and a port with `generate use-case|controller|port`.
* Read `RELEASING.md` before publishing crates.
