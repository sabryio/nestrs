#![allow(missing_docs)]
//! Configurable (instance) modules and global modules.

use std::sync::Arc;

use fx::{App, Error, Export, ModuleDef, ModuleRef, Options};

#[derive(Debug)]
struct DbConfig(&'static str);
#[allow(dead_code)]
struct Repo(Arc<DbConfig>);
struct Feature(Arc<DbConfig>);

fn repo(config: Arc<DbConfig>) -> Arc<Repo> {
    Arc::new(Repo(config))
}

/// `DatabaseModule::for_root(url)`: a configured, global module.
fn for_root(url: &'static str) -> ModuleRef {
    ModuleDef::new("database")
        .global()
        .options(Options::new().supply(DbConfig(url)))
        .export(Export::of::<DbConfig>())
        .into_ref()
}

fn feature_module() -> ModuleRef {
    ModuleDef::new("feature")
        .options(Options::new().constructor(|c: Arc<DbConfig>| Arc::new(Feature(c))))
        .export(Export::of::<Feature>())
        .into_ref()
}

#[tokio::test]
async fn global_module_is_visible_without_importing_it() {
    let mut app = App::new();
    // Imported once at the root, but `feature` never lists it in its imports.
    app = app.options(
        Options::new()
            .module_ref(for_root("pg://a"))
            .module_ref(feature_module()),
    );
    app.start().await.unwrap();
    let feature = app.get::<Feature>().await.unwrap();
    assert_eq!(feature.0.0, "pg://a");
}

#[tokio::test]
async fn non_global_instance_module_stays_private() {
    let hidden = ModuleDef::new("hidden")
        .options(Options::new().supply(DbConfig("x")))
        .export(Export::of::<DbConfig>())
        .into_ref();
    let mut app = App::new().options(
        Options::new()
            .module_ref(hidden)
            .module_ref(feature_module())
            .invoke(|_: Arc<Feature>| {}),
    );
    let error = app.start().await.unwrap_err();
    assert!(
        matches!(error, Error::NotVisible { .. } | Error::Missing { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn cloned_reference_is_one_instance() {
    let shared = for_root("pg://shared");
    let a = ModuleDef::new("a")
        .import(shared.clone())
        .options(Options::new().constructor(repo))
        .export(Export::of::<Repo>())
        .into_ref();
    let b = ModuleDef::new("b").import(shared.clone()).into_ref();
    let mut app = App::new().options(Options::new().module_ref(a).module_ref(b).module_ref(shared));
    app.start().await.unwrap();
    assert!(app.get::<Repo>().await.is_ok());
}

#[tokio::test]
async fn two_instances_providing_one_key_are_reported() {
    let mut app = App::new().options(
        Options::new()
            .module_ref(for_root("a"))
            .module_ref(for_root("b"))
            .constructor(repo)
            .invoke(|_: Arc<Repo>| {}),
    );
    let error = app.start().await.unwrap_err();
    assert!(matches!(error, Error::Ambiguous { .. }), "{error}");
}
