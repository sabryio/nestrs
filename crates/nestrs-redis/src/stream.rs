use std::collections::VecDeque;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use redis::streams::{StreamRangeReply, StreamReadOptions, StreamReadReply};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::{RedisConnection, RedisError};

/// A position in a stream: Redis's own `<milliseconds>-<sequence>` id.
///
/// Ids are totally ordered, assigned by Redis, shared by every instance and stable across
/// restarts, so they are exactly what a client needs to resume after.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId {
    /// Milliseconds part (Redis server time).
    pub ms: u64,
    /// Sequence among entries of the same millisecond.
    pub seq: u64,
}

impl StreamId {
    /// Before every entry.
    pub const START: StreamId = StreamId { ms: 0, seq: 0 };
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.ms, self.seq)
    }
}

impl FromStr for StreamId {
    type Err = ();

    fn from_str(text: &str) -> Result<Self, ()> {
        let (ms, seq) = text.split_once('-').ok_or(())?;
        Ok(Self {
            ms: ms.parse().map_err(|_| ())?,
            seq: seq.parse().map_err(|_| ())?,
        })
    }
}

/// One entry of the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEntry {
    /// Its position.
    pub id: StreamId,
    /// What was published.
    pub payload: Vec<u8>,
}

const FIELD: &str = "p";
/// How many entries one `XRANGE` page fetches while replaying.
const PAGE: usize = 500;

/// A replayable, multi-instance event feed on a Redis Stream.
///
/// - **Publish** appends with `XADD` (trimmed to about `max_len` entries, so memory is bounded).
///   [`publish`](Self::publish) reports errors; [`publish_detached`](Self::publish_detached) never
///   blocks or fails the caller, for use from code that must not wait on Redis: a background
///   worker sends in order and retries briefly.
/// - **Subscribe** returns a [`StreamSubscription`] that first replays what was missed after a
///   cursor (`XRANGE`), then follows live with no gap and no duplicate.
///
/// One reader task per process holds a dedicated connection that blocks on `XREAD` and fans
/// entries out locally, so a thousand SSE clients cost one Redis connection, and
/// the shared connection is never blocked. Redis outages are survived: the reader reconnects
/// and carries on from the last entry it saw, and subscribers that fell behind catch up from
/// the stream.
///
/// Dropping the feed stops its tasks.
pub struct RedisEventStream {
    inner: Arc<Inner>,
    outbox: mpsc::UnboundedSender<Vec<u8>>,
    tasks: Vec<JoinHandle<()>>,
}

struct Inner {
    redis: RedisConnection,
    key: String,
    max_len: usize,
    live: broadcast::Sender<StreamEntry>,
}

impl RedisEventStream {
    /// A feed on the stream `key`, keeping about `max_len` entries for replay. Starts the reader.
    pub async fn new(redis: RedisConnection, key: impl Into<String>, max_len: usize) -> Result<Self, RedisError> {
        let inner = Arc::new(Inner {
            redis,
            key: key.into(),
            max_len: max_len.max(1),
            live: broadcast::channel(1024).0,
        });
        let head = inner.head().await?;
        let (outbox, queue) = mpsc::unbounded_channel();
        let tasks = vec![
            tokio::spawn(read_loop(inner.clone(), head)),
            tokio::spawn(write_loop(inner.clone(), queue)),
        ];
        Ok(Self { inner, outbox, tasks })
    }

    /// Appends `payload`, returning its id.
    pub async fn publish(&self, payload: &[u8]) -> Result<StreamId, RedisError> {
        self.inner.append(payload).await
    }

    /// Queues `payload` to be appended by a background worker, in order with other detached
    /// publishes. Never blocks, never fails; after repeated failures the entry is dropped and logged.
    pub fn publish_detached(&self, payload: Vec<u8>) {
        if self.outbox.send(payload).is_err() {
            tracing::error!("event stream worker has stopped; event dropped");
        }
    }

    /// Starts a subscription. `None` sees entries published after this call returns; `Some(id)`
    /// first replays the retained entries after `id`. A cursor older than the retained history
    /// replays what is still there (best effort).
    pub async fn subscribe(&self, after: Option<StreamId>) -> Result<StreamSubscription, RedisError> {
        // Order matters: take the live receiver before reading anything, so an entry is either
        // in the replay, or arrives live, or both (deduplicated by id), never neither.
        let live = self.inner.live.subscribe();
        let last = match after {
            Some(id) => id,
            None => self.inner.head().await?,
        };
        Ok(StreamSubscription {
            inner: self.inner.clone(),
            live,
            backlog: VecDeque::new(),
            replay_done: after.is_none(),
            last,
        })
    }
}

impl Drop for RedisEventStream {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Inner {
    async fn head(&self) -> Result<StreamId, RedisError> {
        let mut connection = self.redis.handle();
        let reply: StreamRangeReply = redis::cmd("XREVRANGE")
            .arg(&self.key)
            .arg("+")
            .arg("-")
            .arg("COUNT")
            .arg(1)
            .query_async(&mut connection)
            .await?;
        Ok(reply
            .ids
            .first()
            .and_then(|entry| entry.id.parse().ok())
            .unwrap_or(StreamId::START))
    }

    async fn append(&self, payload: &[u8]) -> Result<StreamId, RedisError> {
        let mut connection = self.redis.handle();
        let id: String = redis::cmd("XADD")
            .arg(&self.key)
            .arg("MAXLEN")
            .arg("~")
            .arg(self.max_len)
            .arg("*")
            .arg(FIELD)
            .arg(payload)
            .query_async(&mut connection)
            .await?;
        id.parse()
            .map_err(|()| RedisError::from((redis::ErrorKind::TypeError, "XADD returned a malformed id")))
    }

    /// Entries strictly after `after`, oldest first, one page.
    async fn page_after(&self, after: StreamId) -> Result<Vec<StreamEntry>, RedisError> {
        let mut connection = self.redis.handle();
        let reply: StreamRangeReply = redis::cmd("XRANGE")
            .arg(&self.key)
            .arg(format!("({after}"))
            .arg("+")
            .arg("COUNT")
            .arg(PAGE)
            .query_async(&mut connection)
            .await?;
        Ok(reply.ids.into_iter().filter_map(entry_from).collect())
    }
}

fn entry_from(entry: redis::streams::StreamId) -> Option<StreamEntry> {
    let id = entry.id.parse().ok()?;
    let payload: Vec<u8> = entry.get(FIELD)?;
    Some(StreamEntry { id, payload })
}

/// Follows the stream on a dedicated connection and broadcasts every entry locally.
async fn read_loop(inner: Arc<Inner>, mut last: StreamId) {
    let options = StreamReadOptions::default().block(1000).count(100);
    let mut connection = None;
    loop {
        let live = match &mut connection {
            Some(connection) => connection,
            slot @ None => match inner.redis.dedicated().await {
                Ok(fresh) => slot.insert(fresh),
                Err(error) => {
                    tracing::warn!(%error, "event stream reader cannot connect; retrying");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            },
        };
        let reply: Result<Option<StreamReadReply>, RedisError> =
            redis::AsyncCommands::xread_options(live, &[inner.key.as_str()], &[last.to_string()], &options).await;
        match reply {
            Ok(Some(reply)) => {
                for entry in reply.keys.into_iter().flat_map(|key| key.ids).filter_map(entry_from) {
                    last = entry.id;
                    let _ = inner.live.send(entry); // no local subscribers is fine
                }
            }
            Ok(None) => {} // the block timed out with nothing new
            Err(error) => {
                tracing::warn!(%error, "event stream read failed; reconnecting");
                connection = None;
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// Appends queued payloads in order, retrying briefly.
async fn write_loop(inner: Arc<Inner>, mut queue: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(payload) = queue.recv().await {
        let mut delay = Duration::from_millis(100);
        for attempt in 1..=5 {
            match inner.append(&payload).await {
                Ok(_) => break,
                Err(error) if attempt < 5 => {
                    tracing::warn!(%error, attempt, "event publish failed; retrying");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                Err(error) => tracing::error!(%error, "event publish failed for good; event dropped"),
            }
        }
    }
}

/// One subscriber's view of a [`RedisEventStream`].
pub struct StreamSubscription {
    inner: Arc<Inner>,
    live: broadcast::Receiver<StreamEntry>,
    backlog: VecDeque<StreamEntry>,
    /// Whether the replay of history is finished (a `None` cursor has none).
    replay_done: bool,
    /// The highest id delivered; whatever the replay and the live feed both offer is delivered once.
    last: StreamId,
}

impl StreamSubscription {
    /// The next entry, in id order, each at most once. Waits for one; never returns `None`
    /// while the feed is alive, and `None` once it has been dropped.
    pub async fn next(&mut self) -> Option<StreamEntry> {
        loop {
            if self.backlog.is_empty() && !self.replay_done {
                match self.inner.page_after(self.last).await {
                    Ok(page) => {
                        self.replay_done = page.len() < PAGE;
                        self.backlog = page.into();
                    }
                    Err(error) => {
                        tracing::warn!(%error, "replay failed; retrying");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        continue;
                    }
                }
            }
            let candidate = match self.backlog.pop_front() {
                Some(entry) => entry,
                None => match self.live.recv().await {
                    Ok(entry) => entry,
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        // Too slow for the local channel: catch up from the stream itself.
                        tracing::warn!(missed, "stream subscriber lagged; catching up from the stream");
                        self.replay_done = false;
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
            };
            if candidate.id > self.last {
                self.last = candidate.id;
                return Some(candidate);
            }
        }
    }
}
