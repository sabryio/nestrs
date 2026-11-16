# nestrs-outbox

Transactional outbox for nestrs: record an event in the same database transaction as the data it
describes, and let a relay deliver it afterwards.

- A message exists **iff** the transaction that recorded it committed.
- Delivery is **at least once**; the message id is stable across redeliveries, for deduplication.
- Messages with the same key are delivered one at a time, in order.
- Failures retry with exponential back-off, then become dead letters you can requeue.
- Several relays can share one database; claims keep them apart.

```toml
[dependencies]
nestrs-outbox = "0.1"   # features: sea-orm (default), jobs (default)
```

Part of [nestrs](https://github.com/sabry-awad97/nestrs). Licensed under MIT or Apache-2.0, at your option.
