# nestrs-cli

The `nestrs` command: scaffolds projects and clean-architecture features.

`nestrs new`, `nestrs generate feature|use-case|controller|port|module|job|queue-handler|contract`, `nestrs doctor`. Generators are pure templates producing a plan of file operations; existing Rust files are edited by parsing them, never by text matching.

```sh
cargo install nestrs-cli
nestrs new my-service
cd my-service
nestrs generate feature invoice
nestrs generate use-case invoice archive   # ArchiveInvoice + unit test, registered
nestrs generate port mailer                # domain port + in-memory adapter + module
nestrs generate feature invoice --dry-run  # prints a unified diff, writes nothing
nestrs doctor                              # layer rules; non-zero exit on violations
nestrs generate contract --out web/src/contract.generated.ts   # oRPC contract, read from the Rust sources (--check in CI)
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
