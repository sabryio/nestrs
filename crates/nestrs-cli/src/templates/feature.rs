//! `nestrs generate feature`: one vertical slice through every layer.

use super::fill;
use crate::naming::Names;
use crate::plan::Plan;

const DOMAIN: &str = r#"use nestrs::port;
use uuid::Uuid;

/// Why a __SNAKE__ operation failed. The domain's vocabulary: no HTTP, no SQL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum __PASCAL__Error {
    /// The input breaks a rule of the domain.
    #[error("{0}")]
    Invalid(String),
    /// No __SNAKE__ has this id.
    #[error("no __SNAKE__ with id {0}")]
    NotFound(Uuid),
    /// The store could not be reached or answered with an error.
    #[error("storage failure: {0}")]
    Storage(String),
}

/// A __SNAKE__. Built only through [`new`](Self::new), so an invalid one cannot exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct __PASCAL__ {
    id: Uuid,
    name: String,
}

impl __PASCAL__ {
    /// Creates a __SNAKE__ with a fresh id. The name must be 1–100 characters once trimmed.
    pub fn new(name: &str) -> Result<Self, __PASCAL__Error> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(__PASCAL__Error::Invalid("name must be between 1 and 100 characters".to_owned()));
        }
        Ok(Self {
            id: Uuid::new_v4(),
            name: name.to_owned(),
        })
    }

    /// Its identifier.
    pub fn id(&self) -> Uuid {
        self.id
    }

    /// Its name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Port: persistence for __PLURAL__.
///
/// Defined by the domain, implemented by infrastructure (the dependency points inward).
/// `#[port]` also generates `__PASCAL__RepositoryPort`, the injectable handle. Every
/// implementation honours the same contract: `find` returns `Ok(None)` for an unknown id,
/// `insert` stores the value as given, and `all` returns a stable order.
#[port]
pub trait __PASCAL__Repository {
    /// Looks a __SNAKE__ up by id.
    async fn find(&self, id: Uuid) -> Result<Option<__PASCAL__>, __PASCAL__Error>;
    /// Every __SNAKE__, in a stable order.
    async fn all(&self) -> Result<Vec<__PASCAL__>, __PASCAL__Error>;
    /// Stores a __SNAKE__.
    async fn insert(&self, value: __PASCAL__) -> Result<(), __PASCAL__Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_and_bounded() {
        assert_eq!(__PASCAL__::new("  Ada ").unwrap().name(), "Ada");
        assert!(matches!(__PASCAL__::new("   "), Err(__PASCAL__Error::Invalid(_))));
        assert!(matches!(__PASCAL__::new(&"x".repeat(101)), Err(__PASCAL__Error::Invalid(_))));
    }
}
"#;

const APPLICATION: &str = r#"use nestrs::injectable;
use uuid::Uuid;

use crate::domain::{__PASCAL__, __PASCAL__Error, __PASCAL__RepositoryPort};

/// Use case: create a __SNAKE__.
#[injectable]
pub struct Create__PASCAL__ {
    repository: __PASCAL__RepositoryPort,
}

impl Create__PASCAL__ {
    /// Validates `name` and stores the new __SNAKE__.
    pub async fn execute(&self, name: &str) -> Result<__PASCAL__, __PASCAL__Error> {
        let value = __PASCAL__::new(name)?;
        self.repository.insert(value.clone()).await?;
        Ok(value)
    }
}

/// Use case: fetch one __SNAKE__.
#[injectable]
pub struct Get__PASCAL__ {
    repository: __PASCAL__RepositoryPort,
}

impl Get__PASCAL__ {
    /// Returns the __SNAKE__, or [`__PASCAL__Error::NotFound`].
    pub async fn execute(&self, id: Uuid) -> Result<__PASCAL__, __PASCAL__Error> {
        self.repository.find(id).await?.ok_or(__PASCAL__Error::NotFound(id))
    }
}

/// Use case: list every __SNAKE__.
#[injectable]
pub struct List__PASCAL_PLURAL__ {
    repository: __PASCAL__RepositoryPort,
}

impl List__PASCAL_PLURAL__ {
    /// Returns them in the repository's stable order.
    pub async fn execute(&self) -> Result<Vec<__PASCAL__>, __PASCAL__Error> {
        self.repository.all().await
    }
}
"#;

const INFRASTRUCTURE: &str = r#"use std::sync::RwLock;

use async_trait::async_trait;
use nestrs::prelude::*;
use uuid::Uuid;

use crate::domain::{__PASCAL__, __PASCAL__Error, __PASCAL__Repository, __PASCAL__RepositoryPort};

/// Adapter for [`__PASCAL__Repository`] that keeps __PLURAL__ in process memory.
///
/// Swap it for a database adapter by changing `provides` (or choosing in a `#[provider]`
/// from config); nothing that depends on the port changes.
#[injectable(provides = __PASCAL__RepositoryPort)]
pub struct InMemory__PASCAL__Repository {
    #[inject(default)]
    items: RwLock<Vec<__PASCAL__>>,
}

fn poisoned<T>(_: T) -> __PASCAL__Error {
    __PASCAL__Error::Storage("in-memory store lock poisoned".to_owned())
}

#[async_trait]
impl __PASCAL__Repository for InMemory__PASCAL__Repository {
    async fn find(&self, id: Uuid) -> Result<Option<__PASCAL__>, __PASCAL__Error> {
        Ok(self.items.read().map_err(poisoned)?.iter().find(|item| item.id() == id).cloned())
    }

    async fn all(&self) -> Result<Vec<__PASCAL__>, __PASCAL__Error> {
        let mut items = self.items.read().map_err(poisoned)?.clone();
        items.sort_by_key(__PASCAL__::id);
        Ok(items)
    }

    async fn insert(&self, value: __PASCAL__) -> Result<(), __PASCAL__Error> {
        self.items.write().map_err(poisoned)?.push(value);
        Ok(())
    }
}
"#;

const PRESENTATION: &str = r#"use nestrs::openapi::ToSchema;
use nestrs::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

use crate::application::{Create__PASCAL__, Get__PASCAL__, List__PASCAL_PLURAL__};
use crate::domain::{__PASCAL__, __PASCAL__Error};

/// Request body for `POST /__ROUTE__`.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct Create__PASCAL__Request {
    /// Name, 1–100 characters.
    #[validate(length(min = 1, max = 100, message = "must be between 1 and 100 characters"))]
    #[schema(min_length = 1, max_length = 100)]
    pub name: String,
}

/// A __SNAKE__ as returned to clients. Entities never cross the HTTP boundary directly,
/// so the JSON shape can evolve independently of the domain.
#[derive(Debug, Serialize, ToSchema)]
pub struct __PASCAL__Response {
    /// Identifier.
    pub id: Uuid,
    /// Name.
    pub name: String,
}

impl From<__PASCAL__> for __PASCAL__Response {
    fn from(value: __PASCAL__) -> Self {
        Self {
            id: value.id(),
            name: value.name().to_owned(),
        }
    }
}

/// The single place where this feature's failures meet HTTP status codes.
impl From<__PASCAL__Error> for HttpError {
    fn from(error: __PASCAL__Error) -> Self {
        match error {
            __PASCAL__Error::Invalid(_) => Self::bad_request(error.to_string()),
            __PASCAL__Error::NotFound(_) => Self::not_found(error.to_string()),
            // Storage details are logged by `HttpError`, never sent to the client.
            __PASCAL__Error::Storage(_) => Self::internal(error.to_string()),
        }
    }
}

/// HTTP surface of the __SNAKE__ use cases: translate, delegate, translate back.
#[injectable]
pub struct __PASCAL_PLURAL__Controller {
    create: Arc<Create__PASCAL__>,
    get: Arc<Get__PASCAL__>,
    list: Arc<List__PASCAL_PLURAL__>,
}

#[controller("/__ROUTE__")]
impl __PASCAL_PLURAL__Controller {
    /// List __PLURAL__.
    #[get]
    async fn list(&self) -> Result<Json<Vec<__PASCAL__Response>>, HttpError> {
        let items = self.list.execute().await?;
        Ok(Json(items.into_iter().map(__PASCAL__Response::from).collect()))
    }

    /// Fetch one __SNAKE__ by id.
    #[get("/{id}")]
    #[response(404, "No __SNAKE__ with this id")]
    async fn find(&self, Path(id): Path<Uuid>) -> Result<Json<__PASCAL__Response>, HttpError> {
        Ok(Json(self.get.execute(id).await?.into()))
    }

    /// Create a __SNAKE__.
    #[post]
    #[status(201)]
    async fn create(
        &self,
        ValidatedJson(request): ValidatedJson<Create__PASCAL__Request>,
    ) -> Result<Json<__PASCAL__Response>, HttpError> {
        Ok(Json(self.create.execute(&request.name).await?.into()))
    }
}
"#;

const COMPOSITION: &str = r#"use nestrs::prelude::*;

use crate::application::{Create__PASCAL__, Get__PASCAL__, List__PASCAL_PLURAL__};
use crate::infrastructure::InMemory__PASCAL__Repository;
use crate::presentation::http::__PASCAL_PLURAL__Controller;

/// The __PLURAL__ feature: use cases, the adapter behind the repository port, and the
/// controller. Import it in `AppModule`.
#[module(
    providers   = [InMemory__PASCAL__Repository, Create__PASCAL__, Get__PASCAL__, List__PASCAL_PLURAL__],
    controllers = [__PASCAL_PLURAL__Controller],
)]
pub struct __PASCAL_PLURAL__Module;
"#;

const TEST: &str = r#"//! The __PLURAL__ feature through the real HTTP pipeline, with the in-memory adapter.

use __LIB__::AppModule;
use nestrs::prelude::StatusCode;
use nestrs_testing::TestApp;
use serde_json::{Value, json};

#[tokio::test]
async fn create_then_fetch_and_list() {
    let app = TestApp::start(AppModule).await;
    let client = app.client();

    let created: Value = client
        .post("/__ROUTE__")
        .json(&json!({ "name": "Ada" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(created["name"], "Ada");

    let id = created["id"].as_str().unwrap();
    let fetched: Value = client.get(&format!("/__ROUTE__/{id}")).send().await.assert_status(StatusCode::OK).json();
    assert_eq!(fetched, created);

    let all: Value = client.get("/__ROUTE__").send().await.json();
    assert_eq!(all, json!([created]));
}

#[tokio::test]
async fn bad_input_and_unknown_ids_are_client_errors() {
    let app = TestApp::start(AppModule).await;
    let client = app.client();

    client.post("/__ROUTE__").json(&json!({ "name": "" })).send().await.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
    client
        .get("/__ROUTE__/00000000-0000-0000-0000-000000000000")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}
"#;

/// The plan for a feature: one file per layer, registered in each layer's `mod.rs` and in
/// the app module.
pub fn feature(names: &Names, lib: &str) -> Plan {
    let tokens = [
        ("__PASCAL_PLURAL__", names.pascal_plural.as_str()),
        ("__PASCAL__", names.pascal.as_str()),
        ("__PLURAL__", names.plural.as_str()),
        ("__SNAKE__", names.snake.as_str()),
        ("__ROUTE__", names.route.as_str()),
        ("__LIB__", lib),
    ];
    let (snake, plural, pascal, pascal_plural) = (&names.snake, &names.plural, &names.pascal, &names.pascal_plural);

    let mut plan = Plan::new();
    plan.create(format!("src/domain/{snake}.rs"), fill(DOMAIN, &tokens))
        .create(format!("src/application/{snake}.rs"), fill(APPLICATION, &tokens))
        .create(
            format!("src/infrastructure/memory_{plural}.rs"),
            fill(INFRASTRUCTURE, &tokens),
        )
        .create(
            format!("src/presentation/http/{plural}.rs"),
            fill(PRESENTATION, &tokens),
        )
        .create(
            format!("src/composition/{plural}_module.rs"),
            fill(COMPOSITION, &tokens),
        )
        .create(format!("tests/{plural}.rs"), fill(TEST, &tokens));

    plan.ensure_lines(
        "src/domain/mod.rs",
        [
            format!("mod {snake};"),
            format!("pub use {snake}::{{{pascal}, {pascal}Error, {pascal}Repository, {pascal}RepositoryPort}};"),
        ],
    )
    .ensure_lines(
        "src/application/mod.rs",
        [
            format!("mod {snake};"),
            format!("pub use {snake}::{{Create{pascal}, Get{pascal}, List{pascal_plural}}};"),
        ],
    )
    .ensure_lines(
        "src/infrastructure/mod.rs",
        [
            format!("mod memory_{plural};"),
            format!("pub use memory_{plural}::InMemory{pascal}Repository;"),
        ],
    )
    .ensure_lines(
        "src/presentation/http/mod.rs",
        [
            format!("mod {plural};"),
            format!("pub use {plural}::{{Create{pascal}Request, {pascal}Response, {pascal_plural}Controller}};"),
        ],
    )
    .ensure_lines(
        "src/composition/mod.rs",
        [
            format!("mod {plural}_module;"),
            format!("pub use {plural}_module::{pascal_plural}Module;"),
        ],
    )
    .insert_after(
        "src/composition/app.rs",
        "use nestrs_kit::KitModule;",
        format!("\nuse super::{pascal_plural}Module;"),
    )
    .insert_after(
        "src/composition/app.rs",
        "imports = [",
        format!("{pascal_plural}Module, "),
    );
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Op;

    #[test]
    fn it_creates_one_file_per_layer_and_a_test() {
        let names = Names::parse("invoice", None).unwrap();
        let plan = feature(&names, "shop");
        let created: Vec<String> = plan
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Create { path, .. } => Some(path.display().to_string()),
                _ => None,
            })
            .collect();
        assert_eq!(
            created,
            [
                "src/domain/invoice.rs",
                "src/application/invoice.rs",
                "src/infrastructure/memory_invoices.rs",
                "src/presentation/http/invoices.rs",
                "src/composition/invoices_module.rs",
                "tests/invoices.rs",
            ]
        );
    }

    #[test]
    fn no_placeholder_survives() {
        let names = Names::parse("category", None).unwrap();
        for op in feature(&names, "shop").ops {
            if let Op::Create { path, contents } = op {
                assert!(!contents.contains("__"), "{} still has a placeholder", path.display());
            }
        }
    }
}
