# 1. Start a project

```text
purlin new blog-api --purlin-path ../purlin
cd blog-api
```

The command creates a service that boots and has a passing test:

```text
create   Cargo.toml
create   src/lib.rs
create   src/main.rs
create   src/domain/mod.rs
create   src/application/mod.rs
create   src/infrastructure/mod.rs
create   src/presentation/mod.rs
create   src/presentation/http/mod.rs
create   src/composition/mod.rs
create   src/composition/app.rs
create   tests/smoke.rs
```

## The five layers

| Directory | Holds | May depend on |
|---|---|---|
| `domain/` | entities, value objects, **ports** (traits describing what the domain needs) | nothing else in the project |
| `application/` | use cases: one struct, one `execute` | `domain` |
| `infrastructure/` | **adapters** implementing ports: databases, clients, config | `domain` |
| `presentation/` | the HTTP surface: controllers and DTOs | `application`, `domain` |
| `composition/` | modules that choose and wire everything | all of them |

The arrows only point inward. The domain never learns that HTTP or SQL exist, which is why you can change either without
touching business rules, and why the domain can be tested with no framework at all.

## Make it check itself

Rules that live only in a README erode. `purlin doctor` reads the code (with `syn`, so comments and strings do not count)
and fails when a layer reaches where it must not:

```text
$ purlin doctor
healthy: 9 files follow the layer rules
```

If `presentation/http/posts.rs` ever writes `use crate::infrastructure::...`, the output names the file and line,
explains the rule (`depend on a use case in application, and let composition choose the adapter`), and the exit status is
non-zero. Chapter 9 puts it in CI.

## Generate a slice

```text
purlin generate feature post --dry-run     # prints a unified diff; writes nothing
purlin generate feature post
```

```text
create   src/domain/post.rs
create   src/application/post.rs
create   src/infrastructure/memory_posts.rs
create   src/presentation/http/posts.rs
create   src/composition/posts_module.rs
create   tests/posts.rs
update   src/domain/mod.rs
update   src/application/mod.rs
update   src/infrastructure/mod.rs
update   src/presentation/http/mod.rs
update   src/composition/mod.rs
update   src/composition/app.rs
```

`feature` writes one slice through every layer (entity and repository port, use cases, an in-memory adapter, a controller
with DTOs, a module and an HTTP test) and registers each piece. It never overwrites a file (`--force` to), and running it
twice changes nothing. The rest of the tutorial reads the *finished* blog, which is what you grow toward.

**SOLID here.** The layer table is *dependency inversion* made structural, and `doctor` is its enforcement. Each directory
has one reason to change (*single responsibility*).
