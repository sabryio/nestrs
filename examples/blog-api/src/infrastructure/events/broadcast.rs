use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use purlin::prelude::*;
use tokio::sync::broadcast::{self, error::RecvError};

use crate::domain::{EventCursor, PostEvent, PostEvents, PostSubscription, Sequenced};

/// How many events are kept for replay, and how far a live subscriber may fall
/// behind before it has to catch up from that history.
const RETAINED: usize = 256;

/// In-process, replayable event feed: a bounded history plus a live
/// [`tokio::sync::broadcast`] channel.
///
/// Exact for a single instance. With several, [`RedisPostEvents`](super::RedisPostEvents)
/// shares one feed through Redis Streams; `post_events` picks between them.
#[injectable]
pub struct BroadcastPostEvents {
    #[inject(default)]
    feed: SharedFeed,
}

/// Newtype so the field can be `#[inject(default)]`.
struct SharedFeed(Arc<Feed>);

impl Default for SharedFeed {
    fn default() -> Self {
        // Nanoseconds since the Unix epoch: distinct for every process start.
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        Self(Arc::new(Feed::new(epoch, RETAINED)))
    }
}

/// The shared state. `publish` and `subscribe` both take the lock, which is what
/// makes "replay, then live" gapless: a subscriber's snapshot of the history and its
/// live receiver are created at the same instant, relative to every publish.
struct Feed {
    epoch: u64,
    retain: usize,
    inner: Mutex<Inner>,
    live: broadcast::Sender<Sequenced>,
}

struct Inner {
    /// Sequence number of the next event.
    next_seq: u64,
    /// The most recent events, oldest first.
    history: VecDeque<Sequenced>,
}

impl Feed {
    fn new(epoch: u64, retain: usize) -> Self {
        Self {
            epoch,
            retain,
            inner: Mutex::new(Inner {
                next_seq: 1,
                history: VecDeque::with_capacity(retain),
            }),
            live: broadcast::channel(retain).0,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock only means a publisher panicked mid-update of plain data.
        self.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn publish(&self, event: PostEvent) {
        let mut inner = self.lock();
        let sequenced = Sequenced {
            cursor: EventCursor::new(self.epoch, inner.next_seq),
            event,
        };
        inner.next_seq += 1;
        if inner.history.len() == self.retain {
            inner.history.pop_front();
        }
        inner.history.push_back(sequenced.clone());
        // `Err` only means nobody is listening, which is fine.
        let _ = self.live.send(sequenced);
    }

    fn subscribe(self: &Arc<Self>, after: Option<EventCursor>) -> Subscription {
        let inner = self.lock();
        let head = inner.next_seq - 1;
        // Resume only inside this epoch and never "from the future"; anything else
        // starts at the live edge.
        let baseline = after
            .filter(|cursor| cursor.epoch() == self.epoch)
            .map_or(head, |cursor| cursor.seq().min(head));
        Subscription {
            feed: Arc::downgrade(self),
            backlog: inner
                .history
                .iter()
                .filter(|e| e.cursor.seq() > baseline)
                .cloned()
                .collect(),
            live: self.live.subscribe(),
            last: baseline,
        }
    }

    fn history_after(&self, seq: u64) -> VecDeque<Sequenced> {
        self.lock()
            .history
            .iter()
            .filter(|e| e.cursor.seq() > seq)
            .cloned()
            .collect()
    }
}

#[async_trait]
impl PostEvents for BroadcastPostEvents {
    fn publish(&self, event: PostEvent) {
        self.feed.0.publish(event);
    }

    async fn subscribe(&self, after: Option<EventCursor>) -> Box<dyn PostSubscription> {
        Box::new(self.feed.0.subscribe(after))
    }
}

/// Replays `backlog`, then follows `live`. `last` is the highest sequence number
/// delivered, so whatever either source offers twice is delivered once.
struct Subscription {
    feed: Weak<Feed>,
    backlog: VecDeque<Sequenced>,
    live: broadcast::Receiver<Sequenced>,
    last: u64,
}

impl PostSubscription for Subscription {
    fn next(&mut self) -> Pin<Box<dyn Future<Output = Option<Sequenced>> + Send + '_>> {
        Box::pin(async move {
            loop {
                let candidate = match self.backlog.pop_front() {
                    Some(event) => event,
                    None => match self.live.recv().await {
                        Ok(event) => event,
                        // Fell behind the channel: catch up from the history. Events
                        // the history no longer holds are gone, and that is logged.
                        Err(RecvError::Lagged(missed)) => {
                            let feed = self.feed.upgrade()?;
                            self.backlog = feed.history_after(self.last);
                            tracing::warn!(missed, "event subscriber lagged; catching up from history");
                            continue;
                        }
                        Err(RecvError::Closed) => return None,
                    },
                };
                if candidate.cursor.seq() > self.last {
                    self.last = candidate.cursor.seq();
                    return Some(candidate);
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;

    fn removed() -> (Uuid, PostEvent) {
        let id = Uuid::new_v4();
        (id, PostEvent::Removed(id))
    }

    fn adapter(retain: usize) -> BroadcastPostEvents {
        BroadcastPostEvents {
            feed: SharedFeed(Arc::new(Feed::new(7, retain))),
        }
    }

    async fn seqs(subscription: &mut Box<dyn PostSubscription>, count: usize) -> Vec<u64> {
        let mut out = Vec::new();
        for _ in 0..count {
            out.push(subscription.next().await.expect("an event").cursor.seq());
        }
        out
    }

    #[tokio::test]
    async fn a_fresh_subscription_sees_only_what_comes_after() {
        let events = adapter(16);
        events.publish(removed().1); // before anyone listens
        let mut one = events.subscribe(None).await;
        let mut two = events.subscribe(None).await;
        let (id, event) = removed();
        events.publish(event);
        let first = one.next().await.unwrap();
        assert_eq!(first.event, PostEvent::Removed(id));
        assert_eq!(first.cursor, EventCursor::new(7, 2));
        assert_eq!(two.next().await.unwrap(), first);
    }

    #[tokio::test]
    async fn resuming_replays_what_was_missed_then_goes_live_without_a_seam() {
        let events = adapter(16);
        for _ in 0..4 {
            events.publish(removed().1);
        }
        let mut resumed = events.subscribe(Some(EventCursor::new(7, 2))).await;
        events.publish(removed().1); // live, after the snapshot
        assert_eq!(seqs(&mut resumed, 3).await, [3, 4, 5]);
    }

    #[tokio::test]
    async fn a_cursor_from_another_epoch_or_the_future_replays_nothing() {
        let events = adapter(16);
        events.publish(removed().1);
        events.publish(removed().1);
        let mut other_epoch = events.subscribe(Some(EventCursor::new(99, 1))).await;
        let mut future = events.subscribe(Some(EventCursor::new(7, 500))).await;
        events.publish(removed().1);
        assert_eq!(seqs(&mut other_epoch, 1).await, [3]);
        assert_eq!(seqs(&mut future, 1).await, [3]);
    }

    #[tokio::test]
    async fn history_is_bounded_and_replay_is_best_effort() {
        let events = adapter(3);
        for _ in 0..5 {
            events.publish(removed().1);
        }
        // Events 1 and 2 were evicted; resuming after 0 yields what is still known.
        let mut resumed = events.subscribe(Some(EventCursor::new(7, 0))).await;
        assert_eq!(seqs(&mut resumed, 3).await, [3, 4, 5]);
    }

    #[tokio::test]
    async fn a_lagging_subscriber_catches_up_in_order_without_duplicates() {
        let events = adapter(4);
        let mut slow = events.subscribe(None).await;
        // The channel holds 4; ten events overflow it, the history keeps the last 4.
        for _ in 0..10 {
            events.publish(removed().1);
        }
        let got = seqs(&mut slow, 4).await;
        assert_eq!(got, [7, 8, 9, 10], "caught up from history, in order");
        events.publish(removed().1);
        assert_eq!(seqs(&mut slow, 1).await, [11], "and is live again");
    }

    #[tokio::test]
    async fn the_feed_ends_when_the_publisher_goes_away() {
        let events = adapter(4);
        let mut subscription = events.subscribe(None).await;
        drop(events);
        assert!(subscription.next().await.is_none());
    }
}
