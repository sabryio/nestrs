# Build a service with purlin

A guided tour that builds a blog API (users and posts) the way purlin is meant to be used: a small domain at the centre,
use cases around it, adapters at the edge, and a composition root that wires them.

Every code block that quotes the example comes from `examples/blog-api`, and `cargo test -p purlin-cli --test tutorial`
fails if a quoted block no longer matches its source file, so what you read is what compiles.

| # | Chapter | You learn |
|---|---|---|
| 1 | [Start a project](01-start.md) | `purlin new`, the layout, `purlin doctor` |
| 2 | [The domain](02-domain.md) | value objects, entities, errors, ports |
| 3 | [Use cases](03-use-cases.md) | the application layer, injection, `generate use-case` |
| 4 | [HTTP](04-http.md) | DTOs, one error mapping, controllers, guards |
| 5 | [Wiring](05-wiring.md) | modules, configuration, choosing adapters |
| 6 | [Persistence](06-persistence.md) | SeaORM, migrations, a transaction per request |
| 7 | [Events and observability](07-events-and-observability.md) | the transactional outbox, traces |
| 8 | [Testing](08-testing.md) | black-box tests, swapping providers |
| 9 | [Ship it](09-ship-it.md) | the TypeScript contract, CI gates |

Prerequisites: Rust 1.89 or newer. Install the CLI from a checkout with `cargo install --path crates/purlin-cli`. To follow
along in your own project use `purlin new blog-api --purlin-path <path to the checkout>`; the finished version is
`examples/blog-api`.

The principles behind every chapter are the five SOLID ones; each chapter ends with where they show up.
