# purlin-cli

The `purlin` command: scaffolds projects and clean-architecture features.

`purlin new`, `purlin generate feature|use-case|controller|port|module|job|queue-handler|contract`, `purlin doctor`. Generators are pure templates producing a plan of file operations; existing Rust files are edited by parsing them, never by text matching.

```sh
cargo install purlin-cli
purlin new my-service
cd my-service
purlin generate feature invoice
purlin generate use-case invoice archive   # ArchiveInvoice + unit test, registered
purlin generate port mailer                # domain port + in-memory adapter + module
purlin generate feature invoice --dry-run  # prints a unified diff, writes nothing
purlin doctor                              # layer rules; non-zero exit on violations
purlin generate contract --out web/src/contract.generated.ts   # oRPC contract, read from the Rust sources (--check in CI)
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
