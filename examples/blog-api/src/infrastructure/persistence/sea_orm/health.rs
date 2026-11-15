use nestrs::prelude::*;
use nestrs_health::{HealthIndicator, HealthIndicatorPort, Unhealthy};

use super::Database;

/// Readiness check for the database, contributed to the health set by
/// [`PersistenceModule`](crate::composition::PersistenceModule). Health knows
/// nothing about SeaORM and persistence knows nothing about probes.
#[injectable(contributes = HealthIndicatorPort)]
pub struct DatabaseIndicator {
    database: Arc<Database>,
}

#[async_trait]
impl HealthIndicator for DatabaseIndicator {
    fn name(&self) -> &str {
        "database"
    }

    async fn check(&self) -> Result<(), Unhealthy> {
        // The driver's message can name hosts; say only that it is unreachable.
        self.database.ping().await.map_err(|error| {
            tracing::warn!(%error, "database ping failed");
            Unhealthy::new("database unreachable")
        })
    }
}
