//! Smaller generators that extend an existing project: a use case for an existing feature, a
//! controller, and a port with its first adapter.

use super::fill;
use crate::naming::Names;
use crate::plan::Plan;

const USE_CASE: &str = r#"use purlin::injectable;
use uuid::Uuid;

use crate::domain::{__ENTITY__, __ENTITY__Error, __ENTITY__RepositoryPort};

/// Use case: __VERB__ a __ENTITY_SNAKE__.
///
/// Starts as "load by id"; add the domain behaviour between loading and returning (and a
/// repository call when the result must be stored). Controllers call [`execute`](Self::execute)
/// and never touch the repository themselves.
#[injectable]
pub struct __USE_CASE__ {
    repository: __ENTITY__RepositoryPort,
}

impl __USE_CASE__ {
    /// Returns the __ENTITY_SNAKE__, or [`__ENTITY__Error::NotFound`].
    pub async fn execute(&self, id: Uuid) -> Result<__ENTITY__, __ENTITY__Error> {
        self.repository.find(id).await?.ok_or(__ENTITY__Error::NotFound(id))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use super::*;
    use crate::domain::__ENTITY__Repository;

    /// Knows exactly one __ENTITY_SNAKE__.
    struct One(__ENTITY__);

    #[async_trait]
    impl __ENTITY__Repository for One {
        async fn find(&self, id: Uuid) -> Result<Option<__ENTITY__>, __ENTITY__Error> {
            Ok((self.0.id() == id).then(|| self.0.clone()))
        }
        async fn all(&self) -> Result<Vec<__ENTITY__>, __ENTITY__Error> {
            Ok(vec![self.0.clone()])
        }
        async fn insert(&self, _: __ENTITY__) -> Result<(), __ENTITY__Error> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn it_returns_the_known_one_and_not_found_for_others() {
        let known = __ENTITY__::new("Ada").unwrap();
        let use_case = __USE_CASE__ {
            repository: Arc::new(One(known.clone())),
        };
        assert_eq!(use_case.execute(known.id()).await.unwrap(), known);
        let other = Uuid::new_v4();
        assert_eq!(use_case.execute(other).await, Err(__ENTITY__Error::NotFound(other)));
    }
}
"#;

const CONTROLLER: &str = r#"use purlin::openapi::ToSchema;
use purlin::prelude::*;
use serde::Serialize;

/// Response of `GET /__ROUTE__`.
#[derive(Debug, Serialize, ToSchema)]
pub struct __PASCAL__Response {
    /// What the endpoint reports.
    pub message: String,
}

/// HTTP surface of __SNAKE__: translate, delegate to use cases, translate back.
///
/// Inject use cases as fields (`Arc<TheUseCase>`); keep decisions out of handlers.
#[injectable]
pub struct __PASCAL__Controller;

#[controller("/__ROUTE__")]
impl __PASCAL__Controller {
    /// Describe what this endpoint returns.
    #[get]
    async fn index(&self) -> Json<__PASCAL__Response> {
        Json(__PASCAL__Response {
            message: "__SNAKE__".to_owned(),
        })
    }
}
"#;

const PORT: &str = r#"use purlin::port;

/// Why a __SNAKE__ call failed. The domain's vocabulary: no HTTP, no SDK error types.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum __PASCAL__Error {
    /// The other side refused the request.
    #[error("{0}")]
    Rejected(String),
    /// The other side could not be reached.
    #[error("__SNAKE__ unavailable: {0}")]
    Unavailable(String),
}

/// Port: what the application needs from __SNAKE__.
///
/// Defined here, implemented in `infrastructure` (the dependency points inward). `#[port]` also
/// generates `__PASCAL__Port`, the injectable handle. Replace `send` with the operations your use
/// cases need; every implementation must honour the same contract: `Ok(())` means it was accepted.
#[port]
pub trait __PASCAL__ {
    /// Hands `message` over.
    async fn send(&self, message: &str) -> Result<(), __PASCAL__Error>;
}
"#;

const ADAPTER: &str = r#"use std::sync::Mutex;

use async_trait::async_trait;
use purlin::prelude::*;

use crate::domain::{__PASCAL__, __PASCAL__Error, __PASCAL__Port};

/// Adapter for [`__PASCAL__`] that records what it was given instead of calling anything.
///
/// Fine for development and tests. Swap it for the real client by changing `provides` (or
/// choosing in a `#[provider]` from config); nothing that depends on the port changes.
#[injectable(provides = __PASCAL__Port)]
pub struct InMemory__PASCAL__ {
    #[inject(default)]
    sent: Mutex<Vec<String>>,
}

impl InMemory__PASCAL__ {
    /// Everything sent so far, oldest first.
    pub fn sent(&self) -> Vec<String> {
        self.sent.lock().map(|sent| sent.clone()).unwrap_or_default()
    }
}

#[async_trait]
impl __PASCAL__ for InMemory__PASCAL__ {
    async fn send(&self, message: &str) -> Result<(), __PASCAL__Error> {
        self.sent
            .lock()
            .map_err(|_| __PASCAL__Error::Unavailable("in-memory store lock poisoned".to_owned()))?
            .push(message.to_owned());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn it_records_in_order() {
        let adapter = InMemory__PASCAL__ { sent: Mutex::default() };
        adapter.send("one").await.unwrap();
        adapter.send("two").await.unwrap();
        assert_eq!(adapter.sent(), ["one", "two"]);
    }
}
"#;

const PORT_MODULE: &str = r#"use purlin::prelude::*;

use crate::domain::__PASCAL__Port;
use crate::infrastructure::InMemory__PASCAL__;

/// The __SNAKE__ port and its adapter. Import it where something needs `__PASCAL__Port`.
#[module(
    providers = [InMemory__PASCAL__],
    exports   = [__PASCAL__Port],
)]
pub struct __PASCAL__Module;
"#;

/// A use case for an existing feature: `src/application/<action>_<feature>.rs`, exported from
/// `application`, provided by the feature's module (`src/composition/<plural>_module.rs`).
pub fn use_case(feature: &Names, action: &Names) -> Plan {
    let use_case = format!("{}{}", action.pascal, feature.pascal);
    let file = format!("{}_{}", action.snake, feature.snake);
    let tokens = [
        ("__USE_CASE__", use_case.as_str()),
        ("__ENTITY_SNAKE__", feature.snake.as_str()),
        ("__ENTITY__", feature.pascal.as_str()),
        ("__VERB__", action.snake.as_str()),
    ];
    let mut plan = Plan::new();
    plan.create(format!("src/application/{file}.rs"), fill(USE_CASE, &tokens))
        .ensure_lines(
            "src/application/mod.rs",
            [format!("mod {file};"), format!("pub use {file}::{use_case};")],
        )
        .patch_module(
            format!("src/composition/{}_module.rs", feature.plural),
            "providers",
            &use_case,
            &use_case,
            Some(&format!("crate::application::{use_case}")),
        );
    plan
}

/// A controller in `presentation/http`, registered in `module_stem`'s module when that exists.
pub fn controller(names: &Names, module_stem: &str) -> Plan {
    let (snake, pascal) = (&names.snake, &names.pascal);
    let tokens = [
        ("__PASCAL__", pascal.as_str()),
        ("__SNAKE__", snake.as_str()),
        ("__ROUTE__", names.route.as_str()),
    ];
    let controller = format!("{pascal}Controller");
    let mut plan = Plan::new();
    plan.create(format!("src/presentation/http/{snake}.rs"), fill(CONTROLLER, &tokens))
        .ensure_lines(
            "src/presentation/http/mod.rs",
            [
                format!("mod {snake};"),
                format!("pub use {snake}::{{{controller}, {pascal}Response}};"),
            ],
        )
        .patch_module(
            format!("src/composition/{module_stem}_module.rs"),
            "controllers",
            &controller,
            &controller,
            Some(&format!("crate::presentation::http::{controller}")),
        );
    plan
}

/// A port in `domain`, an in-memory adapter in `infrastructure`, and a module that provides and
/// exports it, imported by `AppModule`.
pub fn port(names: &Names) -> Plan {
    let (snake, pascal) = (&names.snake, &names.pascal);
    let tokens = [("__PASCAL__", pascal.as_str()), ("__SNAKE__", snake.as_str())];
    let mut plan = Plan::new();
    plan.create(format!("src/domain/{snake}.rs"), fill(PORT, &tokens))
        .create(format!("src/infrastructure/memory_{snake}.rs"), fill(ADAPTER, &tokens))
        .create(format!("src/composition/{snake}_module.rs"), fill(PORT_MODULE, &tokens))
        .ensure_lines(
            "src/domain/mod.rs",
            [
                format!("mod {snake};"),
                format!("pub use {snake}::{{{pascal}, {pascal}Error, {pascal}Port}};"),
            ],
        )
        .ensure_lines(
            "src/infrastructure/mod.rs",
            [
                format!("mod memory_{snake};"),
                format!("pub use memory_{snake}::InMemory{pascal};"),
            ],
        )
        .ensure_lines(
            "src/composition/mod.rs",
            [
                format!("mod {snake}_module;"),
                format!("pub use {snake}_module::{pascal}Module;"),
            ],
        )
        .import_module(
            "src/composition/app.rs",
            &format!("{pascal}Module"),
            &format!("super::{pascal}Module"),
        );
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Op;

    fn created(plan: &Plan) -> Vec<String> {
        plan.ops
            .iter()
            .filter_map(|op| match op {
                Op::Create { path, .. } => Some(path.display().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_use_case_is_named_action_then_entity() {
        let plan = use_case(
            &Names::parse("invoice", None).unwrap(),
            &Names::parse("archive", None).unwrap(),
        );
        assert_eq!(created(&plan), ["src/application/archive_invoice.rs"]);
        let Op::Create { contents, .. } = &plan.ops[0] else {
            panic!()
        };
        assert!(contents.contains("pub struct ArchiveInvoice") && contents.contains("InvoiceRepositoryPort"));
    }

    #[test]
    fn a_controller_and_a_port_create_their_layers() {
        let names = Names::parse("billing", None).unwrap();
        assert_eq!(
            created(&controller(&names, "billing")),
            ["src/presentation/http/billing.rs"]
        );
        assert_eq!(
            created(&port(&Names::parse("mailer", None).unwrap())),
            [
                "src/domain/mailer.rs",
                "src/infrastructure/memory_mailer.rs",
                "src/composition/mailer_module.rs"
            ]
        );
    }

    #[test]
    fn no_placeholder_survives() {
        let names = Names::parse("mailer", None).unwrap();
        let plans = [
            port(&names),
            controller(&names, "mailer"),
            use_case(&names, &Names::parse("archive", None).unwrap()),
        ];
        for plan in plans {
            for op in plan.ops {
                if let Op::Create { path, contents } = op {
                    assert!(!contents.contains("__"), "{} still has a placeholder", path.display());
                }
            }
        }
    }
}
