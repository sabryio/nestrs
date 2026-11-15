use syn::{Receiver, ReceiverKind};

/// Whether the receiver is exactly `&self` (not `self`, `&mut self`, `mut self` or
/// `self: Type`).
pub(crate) fn is_shared_self(receiver: &Receiver) -> bool {
    receiver.mutability.is_none() && matches!(receiver.kind, ReceiverKind::Reference(_, _, None))
}
