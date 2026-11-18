use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use purlin::port;
use uuid::Uuid;

use super::Post;

/// Something that happened to a post. The domain's vocabulary for "what changed";
/// transports decide how to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostEvent {
    /// A post was published.
    Published(Post),
    /// A post was deleted.
    Removed(Uuid),
}

/// A position in the event feed: the feed's `epoch` (one per process start) and the
/// event's sequence number within it.
///
/// Subscribers keep the last cursor they saw and hand it back to resume. The epoch
/// is what makes that safe across restarts: sequence numbers start over in a new
/// process, so a cursor from an old epoch must never be mistaken for a position in
/// the new one.
///
/// Text form: `"<epoch>-<seq>"`, e.g. `"1731312000-42"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventCursor {
    epoch: u64,
    seq: u64,
}

impl EventCursor {
    /// A cursor at `seq` within `epoch`.
    pub fn new(epoch: u64, seq: u64) -> Self {
        Self { epoch, seq }
    }

    /// The feed instance this position belongs to.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// The event's number within its epoch, starting at 1.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

impl fmt::Display for EventCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.epoch, self.seq)
    }
}

/// A cursor text that is not `"<epoch>-<seq>"`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("`{0}` is not an event cursor")]
pub struct InvalidCursor(String);

impl FromStr for EventCursor {
    type Err = InvalidCursor;

    fn from_str(text: &str) -> Result<Self, InvalidCursor> {
        let invalid = || InvalidCursor(text.to_owned());
        let (epoch, seq) = text.split_once('-').ok_or_else(invalid)?;
        Ok(Self::new(
            epoch.parse().map_err(|_| invalid())?,
            seq.parse().map_err(|_| invalid())?,
        ))
    }
}

/// An event together with its position in the feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sequenced {
    /// Where the event sits in the feed; hand it back to resume after it.
    pub cursor: EventCursor,
    /// What happened.
    pub event: PostEvent,
}

/// One subscriber's view of the event feed.
///
/// A hand-rolled async iterator so the domain needs neither `futures` nor a
/// runtime: `next` resolves with the next event, or `None` once the feed is over.
///
/// Contract: events arrive in cursor order, each at most once.
pub trait PostSubscription: Send {
    /// The next event, or `None` when the feed has ended.
    fn next(&mut self) -> Pin<Box<dyn Future<Output = Option<Sequenced>> + Send + '_>>;
}

/// Port: a replayable feed of post events.
///
/// Contract:
/// - `publish` never blocks and never fails (having no subscribers is normal);
/// - `subscribe(None)` sees only events published after the call returns (it is async so an
///   adapter can ask its backing store where the live edge is, exactly);
/// - `subscribe(Some(cursor))` first replays the retained events *after* `cursor`,
///   then continues live, with no gap and no duplicate between the two. A cursor
///   from another epoch, or older than the retained history, replays what is still
///   known and nothing more: replay is best effort, callers that need certainty
///   refetch the underlying data.
#[port]
pub trait PostEvents {
    /// Announces an event to every current subscriber and records it for replay.
    fn publish(&self, event: PostEvent);
    /// Starts a subscription, optionally resuming after a cursor.
    async fn subscribe(&self, after: Option<EventCursor>) -> Box<dyn PostSubscription>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip_and_reject_garbage() {
        let cursor = EventCursor::new(1731312000, 42);
        assert_eq!(cursor.to_string(), "1731312000-42");
        assert_eq!("1731312000-42".parse(), Ok(cursor));
        for bad in ["", "42", "a-b", "1-", "-1", "1-2-3", "1 - 2"] {
            assert!(bad.parse::<EventCursor>().is_err(), "{bad:?} should not parse");
        }
    }
}
