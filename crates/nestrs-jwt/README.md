# nestrs-jwt

JWT bearer-token authenticator for nestrs.

`JwtAuthenticator` validates bearer tokens (signature, expiry, issuer, audience) from static or rotating key sources and maps claims to a `Principal`.

Used by `nestrs-http`'s `BearerAuth` guard.

```toml
[dependencies]
nestrs-jwt = "0.1"
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
