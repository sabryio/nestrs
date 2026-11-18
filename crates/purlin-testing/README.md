# purlin-testing

Test harness for purlin: provider overrides and an in-process HTTP client.

Builds the real module graph, swaps chosen providers for fakes and sends requests without opening a socket.

Add it under `[dev-dependencies]`.

```toml
[dependencies]
purlin-testing = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
