# nestrs-cli

The `nestrs` command: scaffolds projects and clean-architecture features.

`nestrs new`, `nestrs generate feature|module|job|queue-handler`. Generators are pure templates producing a plan of file operations; existing Rust files are edited by parsing them, never by text matching.

```sh
cargo install nestrs-cli
nestrs new my-service
cd my-service
nestrs generate feature invoice
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
