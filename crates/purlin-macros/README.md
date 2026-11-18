# purlin-macros

Attribute macros for purlin: `#[injectable]`, `#[controller]`, `#[module]`, `#[port]`.

Proc macros only. They expand to code that refers to `::purlin::..`, so they are used through the `purlin` crate.

Not meant to be depended on directly.

```toml
[dependencies]
purlin-macros = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
