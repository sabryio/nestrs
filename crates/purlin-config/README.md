# purlin-config

Layered, typed, validated configuration: defaults, files, environment.

Deserialises into your own struct and validates it with `validator`, so a bad configuration fails at startup with a readable error.

Re-exported by `purlin` as `purlin::config`.

```toml
[dependencies]
purlin-config = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
