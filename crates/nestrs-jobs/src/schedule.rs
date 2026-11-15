//! When a job runs.

use std::time::Duration;

use chrono::{DateTime, Utc};

/// A schedule that could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    /// The cron expression did not parse.
    #[error("invalid cron expression {expression:?}: {reason}")]
    Cron {
        /// What was given.
        expression: String,
        /// Why it was refused.
        reason: String,
    },
    /// An interval of zero would spin.
    #[error("an interval must be longer than zero")]
    ZeroInterval,
}

/// When a [`Job`](crate::Job) fires.
///
/// Cron expressions use the seven-field `sec min hour day-of-month month day-of-week [year]`
/// form and are evaluated in UTC, so every instance agrees on when a tick happens.
#[derive(Debug, Clone)]
pub struct Schedule(Kind);

#[derive(Debug, Clone)]
enum Kind {
    Every(Duration),
    Cron(Box<cron::Schedule>),
}

impl Schedule {
    /// Every `period`, measured from the end of the previous run.
    pub fn every(period: Duration) -> Result<Self, ScheduleError> {
        if period.is_zero() {
            return Err(ScheduleError::ZeroInterval);
        }
        Ok(Self(Kind::Every(period)))
    }

    /// On a cron expression, such as `"0 */5 * * * *"` (every five minutes).
    pub fn cron(expression: &str) -> Result<Self, ScheduleError> {
        expression
            .parse::<cron::Schedule>()
            .map(|schedule| Self(Kind::Cron(Box::new(schedule))))
            .map_err(|error| ScheduleError::Cron {
                expression: expression.to_owned(),
                reason: error.to_string(),
            })
    }

    /// How long to wait from `now` until the next run. `None` when the schedule has
    /// no future occurrence (a cron with a past year).
    pub fn next_delay(&self, now: DateTime<Utc>) -> Option<Duration> {
        match &self.0 {
            Kind::Every(period) => Some(*period),
            Kind::Cron(schedule) => {
                let next = schedule.after(&now).next()?;
                Some((next - now).to_std().unwrap_or(Duration::ZERO))
            }
        }
    }

    /// The gap between consecutive runs around `now`; used to size the cross-instance
    /// lock so one tick is claimed once but the next tick is free.
    pub fn period(&self, now: DateTime<Utc>) -> Duration {
        match &self.0 {
            Kind::Every(period) => *period,
            Kind::Cron(schedule) => {
                let mut upcoming = schedule.after(&now);
                match (upcoming.next(), upcoming.next()) {
                    (Some(first), Some(second)) => (second - first).to_std().unwrap_or(Duration::from_secs(1)),
                    _ => Duration::from_secs(60),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(h: u32, m: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, h, m, s).unwrap()
    }

    #[test]
    fn an_interval_waits_the_period() {
        let schedule = Schedule::every(Duration::from_secs(30)).unwrap();
        assert_eq!(schedule.next_delay(at(0, 0, 0)), Some(Duration::from_secs(30)));
    }

    #[test]
    fn a_zero_interval_is_refused() {
        assert_eq!(
            Schedule::every(Duration::ZERO).unwrap_err(),
            ScheduleError::ZeroInterval
        );
    }

    #[test]
    fn cron_waits_until_the_next_boundary() {
        let schedule = Schedule::cron("0 */5 * * * *").unwrap();
        assert_eq!(schedule.next_delay(at(10, 2, 30)), Some(Duration::from_secs(150)));
        assert_eq!(schedule.period(at(10, 2, 30)), Duration::from_secs(300));
    }

    #[test]
    fn a_bad_expression_names_itself() {
        let error = Schedule::cron("every tuesday").unwrap_err();
        assert!(error.to_string().contains("every tuesday"), "{error}");
    }
}
