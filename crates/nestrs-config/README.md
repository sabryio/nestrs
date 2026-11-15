# nestrs-config

Layered, typed, validated configuration: defaults, files, environment.

Deserialises into your own struct and validates it with `validator`, so a bad configuration fails at startup with a readable error.

Re-exported by `nestrs` as `nestrs::config`.

```toml
[dependencies]
nestrs-config = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
