# nestrs-kit

Reusable building blocks for nestrs services: timing, error logging, request context, pagination and a server bootstrap.

The pieces nearly every service rewrites: `TimingInterceptor`, `ErrorLogFilter`, `RequestContext`, pagination (`PageQuery`, `PaginationPipe`) and a `KitModule` that registers them.

Optional; take what you need.

```toml
[dependencies]
nestrs-kit = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
