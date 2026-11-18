# purlin-jwt

JWT bearer-token authenticator for purlin.

`JwtAuthenticator` validates bearer tokens (signature, expiry, issuer, audience) from static or rotating key sources and maps claims to a `Principal`.

Used by `purlin-http`'s `BearerAuth` guard.

```toml
[dependencies]
purlin-jwt = "0.1"
```

Part of [purlin](https://github.com/sabry-awad97/purlin). Licensed under MIT or Apache-2.0, at your option.
