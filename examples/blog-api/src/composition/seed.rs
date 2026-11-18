//! Start-up seeding, driven by lifecycle hooks. It drives use cases, so it is wiring (composition),
//! not an adapter behind a port.

use std::sync::Arc;

use purlin::prelude::*;

use crate::application::{PublishPost, PublishPostCommand, RegisterUser, RegisterUserCommand};
use crate::infrastructure::config::AppConfig;

/// Inserts a demo author and post at start-up when `BLOG_SEED_DEMO_DATA=true`, and
/// logs when the application shuts down.
///
/// It exists to show the lifecycle attributes: `#[on_init]` runs after the whole
/// graph is built, `#[on_destroy]` at graceful shutdown. It drives the real use
/// cases (injected like anything else), so seeded data obeys the same rules as any
/// request.
#[injectable]
pub struct DemoDataSeeder {
    config: Arc<AppConfig>,
    register_user: Arc<RegisterUser>,
    publish_post: Arc<PublishPost>,
}

#[lifecycle]
impl DemoDataSeeder {
    #[on_init]
    async fn seed(&self) -> Result<(), String> {
        if !self.config.seed_demo_data {
            return Ok(());
        }
        let author = self
            .register_user
            .execute(RegisterUserCommand {
                name: "Demo Author".to_owned(),
                email: "demo@example.com".to_owned(),
            })
            .await
            .map_err(|error| error.to_string())?;
        self.publish_post
            .execute(PublishPostCommand {
                author_id: author.id(),
                title: "Hello from the seeder".to_owned(),
                body: "This post was created by an #[on_init] hook.".to_owned(),
            })
            .await
            .map_err(|error| error.to_string())?;
        tracing::info!(author = %author.id(), "seeded demo data");
        Ok(())
    }

    #[on_destroy]
    async fn stopped(&self) -> Result<(), String> {
        tracing::info!("demo data seeder stopped");
        Ok(())
    }
}
