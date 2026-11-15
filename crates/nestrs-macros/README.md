# nestrs-macros

Attribute macros for nestrs: `#[injectable]`, `#[controller]`, `#[module]`, `#[port]`.

Proc macros only. They expand to code that refers to `::nestrs::..`, so they are used through the `nestrs` crate.

Not meant to be depended on directly.

```toml
[dependencies]
nestrs-macros = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
