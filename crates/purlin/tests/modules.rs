//! `#[module]` with configurable and global modules, and `App::builder` overrides.

use http_body_util::BodyExt;
use purlin::http::axum::body::Body;
use purlin::http::axum::http::{Method, Request};
use purlin::prelude::*;
use tower::ServiceExt;

// ------------------------------------------------------------ configurable

struct Settings {
    greeting: String,
}

struct SettingsModule;
impl SettingsModule {
    fn for_root(greeting: &str) -> ModuleRef {
        ModuleDef::new("SettingsModule")
            .global()
            .providers([Provider::value(Settings {
                greeting: greeting.to_owned(),
            })])
            .exports([Token::of::<Settings>()])
            .into_ref()
    }
}

#[injectable]
struct Greeter {
    settings: Arc<Settings>,
}

#[injectable]
struct HelloController {
    greeter: Arc<Greeter>,
}

#[controller("/hello")]
impl HelloController {
    #[get]
    async fn hello(&self) -> String {
        self.greeter.settings.greeting.clone()
    }
}

/// Does not import `SettingsModule`: it is global.
#[module(providers = [Greeter], controllers = [HelloController])]
struct FeatureModule;

#[module(imports = [SettingsModule::for_root("hello from config"), FeatureModule])]
struct AppModule;

async fn get(router: &purlin::http::axum::Router, uri: &str) -> String {
    let request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    String::from_utf8(response.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap()
}

#[tokio::test]
async fn expression_imports_and_global_modules_work_through_the_macro() {
    let router = App::create(AppModule).await.unwrap().into_router();
    assert_eq!(get(&router, "/hello").await, "hello from config");
}

#[tokio::test]
async fn overrides_replace_a_provider_for_its_dependents() {
    let router = App::builder(AppModule)
        .override_provider(Provider::value(Settings {
            greeting: "overridden".to_owned(),
        }))
        .build()
        .await
        .unwrap()
        .into_router();
    assert_eq!(get(&router, "/hello").await, "overridden");
}

#[tokio::test]
async fn prepared_override_sets_can_be_applied_and_the_last_one_wins() {
    let first = Overrides::new().provide(Provider::value(Settings {
        greeting: "first".to_owned(),
    }));
    let second = Overrides::new().provide(Provider::value(Settings {
        greeting: "second".to_owned(),
    }));
    let router = App::builder(AppModule)
        .overrides(first)
        .overrides(second)
        .build()
        .await
        .unwrap()
        .into_router();
    assert_eq!(get(&router, "/hello").await, "second");
}

// ------------------------------------------------------- the `global` flag

#[module(global, providers = [Settings2], exports = [Settings2])]
struct FlagGlobal;

#[injectable]
struct Settings2;

#[injectable]
struct UsesSettings2 {
    _settings: Arc<Settings2>,
}

#[module(providers = [UsesSettings2])]
struct Consumer;

#[module(imports = [FlagGlobal, Consumer])]
struct FlagApp;

#[tokio::test]
async fn the_global_flag_on_the_attribute_makes_exports_visible_everywhere() {
    let app = App::create(FlagApp).await.unwrap();
    assert!(app.container().resolve::<UsesSettings2>().is_ok());
}

#[module(global = false, providers = [Settings2], exports = [Settings2])]
struct ExplicitlyLocal;

#[module(imports = [ExplicitlyLocal, Consumer])]
struct LocalApp;

#[tokio::test]
async fn global_equals_false_keeps_the_module_local() {
    let error = App::create(LocalApp).await.unwrap_err();
    assert!(
        matches!(error, AppError::Di(DiError::NotVisible { module: "Consumer", .. })),
        "{error}"
    );
}
