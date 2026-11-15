use nestrs::prelude::*;

#[port]
pub trait Mailer {
    async fn send(&self, to: &str) -> Result<(), String>;
}

#[injectable(provides = MailerPort)]
pub struct LogMailer;

#[async_trait]
impl Mailer for LogMailer {
    async fn send(&self, _to: &str) -> Result<(), String> {
        Ok(())
    }
}

#[injectable]
pub struct Welcome {
    mailer: MailerPort,
}

#[injectable(scope = request)]
pub struct Visit;

#[provider]
fn greeting() -> String {
    "hello".to_owned()
}

#[module(providers = [LogMailer, Welcome, Visit, greeting], exports = [MailerPort, Welcome])]
pub struct MailModule;

fn main() {}
