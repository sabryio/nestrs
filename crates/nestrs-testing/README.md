# nestrs-testing

Test harness for nestrs: provider overrides and an in-process HTTP client.

Builds the real module graph, swaps chosen providers for fakes and sends requests without opening a socket.

Add it under `[dev-dependencies]`.

```toml
[dependencies]
nestrs-testing = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
