//! Generators: pure functions from names to a [`Plan`](crate::Plan). They never touch a
//! disk, so each is tested by reading the plan it returns.

mod feature;
mod jobs;
mod project;
mod slices;
mod worker;

pub use feature::feature;
pub use jobs::{job, module, queue_handler};
pub use project::{DependencySource, project};
pub use slices::{controller, port, use_case};
pub use worker::worker;

/// Replaces every `(token, value)` in `template`.
pub(crate) fn fill(template: &str, tokens: &[(&str, &str)]) -> String {
    tokens
        .iter()
        .fold(template.to_owned(), |text, (token, value)| text.replace(token, value))
}
