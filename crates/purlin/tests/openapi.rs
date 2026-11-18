//! OpenAPI generation from `#[controller]` declarations.

use http_body_util::BodyExt;
use purlin::http::axum::body::Body;
use purlin::http::axum::http::Request;
use purlin::openapi::{ApiInfo, IntoParams, ToSchema};
use purlin::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tower::ServiceExt;
use uuid::Uuid;
use validator::Validate;

#[derive(Deserialize, Validate, ToSchema)]
struct NewBook {
    #[validate(length(min = 1))]
    title: String,
}

#[derive(Serialize, ToSchema)]
struct Book {
    id: Uuid,
    title: String,
}

#[derive(Deserialize, IntoParams)]
struct Filter {
    /// Only books whose title contains this text.
    #[allow(dead_code)]
    q: Option<String>,
}

/// Has no schema derive: documented without one rather than failing to compile.
#[derive(Serialize, Deserialize)]
struct Untyped {
    anything: String,
}

#[injectable]
struct Key;
impl Guard for Key {
    async fn check(&self, _parts: &mut Parts) -> Result<(), HttpError> {
        Ok(())
    }
}

#[injectable]
struct Maybe;
impl Guard for Maybe {
    const ANONYMOUS_OK: bool = true;

    async fn check(&self, _parts: &mut Parts) -> Result<(), HttpError> {
        Ok(())
    }
}

#[injectable]
struct BooksController;

#[controller("/books")]
impl BooksController {
    /// List books.
    ///
    /// Supports a free-text filter.
    #[get]
    async fn list(&self, Query(_filter): Query<Filter>) -> Json<Vec<Book>> {
        Json(Vec::new())
    }

    /// Fetch one book.
    #[get("/{id}")]
    async fn find(&self, Path(id): Path<Uuid>) -> Result<Json<Book>, HttpError> {
        Ok(Json(Book {
            id,
            title: String::new(),
        }))
    }

    /// Add a book.
    #[post]
    #[guard(Key)]
    async fn create(&self, ValidatedJson(new): ValidatedJson<NewBook>) -> Result<(StatusCode, Json<Book>), HttpError> {
        Ok((
            StatusCode::CREATED,
            Json(Book {
                id: Uuid::nil(),
                title: new.title,
            }),
        ))
    }

    #[delete("/{id}")]
    #[guard(Key)]
    #[roles("admin", "librarian")]
    #[permissions("books:delete")]
    async fn remove(&self, Path(_id): Path<Uuid>) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    /// The caller's books, personalised when signed in.
    #[get("/mine")]
    #[guard(Maybe)]
    async fn mine(&self) -> Json<Vec<Book>> {
        Json(Vec::new())
    }

    #[post("/loose")]
    async fn loose(&self, Json(body): Json<Untyped>) -> Json<Untyped> {
        Json(body)
    }
}

#[module(providers = [Key, Maybe], controllers = [BooksController])]
struct Library;

async fn spec() -> Value {
    let app = App::create(Library).await.unwrap();
    serde_json::to_value(app.openapi(&ApiInfo::new("Library", "1.2.3").description("books"))).unwrap()
}

#[tokio::test]
async fn paths_methods_summaries_and_tags_come_from_the_controller() {
    let spec = spec().await;
    assert_eq!(spec["info"]["title"], "Library");
    assert_eq!(spec["info"]["version"], "1.2.3");

    let list = &spec["paths"]["/books"]["get"];
    assert_eq!(list["summary"], "List books.");
    assert_eq!(list["description"], "Supports a free-text filter.");
    assert_eq!(list["tags"][0], "BooksController");
    assert_eq!(list["operationId"], "BooksController_list");
    assert_eq!(
        list["responses"]["200"]["content"]["application/json"]["schema"]["type"],
        "array"
    );

    assert!(spec["paths"]["/books/{id}"]["get"].is_object());
    assert!(spec["paths"]["/books/{id}"]["delete"].is_object());
    assert!(spec["paths"]["/books"]["post"].is_object());
}

#[tokio::test]
async fn parameters_bodies_and_responses_are_derived_from_the_signature() {
    let spec = spec().await;

    let query = &spec["paths"]["/books"]["get"]["parameters"][0];
    assert_eq!(
        (query["name"].as_str(), query["in"].as_str()),
        (Some("q"), Some("query"))
    );

    let id = &spec["paths"]["/books/{id}"]["get"]["parameters"][0];
    assert_eq!((id["name"].as_str(), id["in"].as_str()), (Some("id"), Some("path")));
    assert_eq!(id["schema"]["format"], "uuid");

    let create = &spec["paths"]["/books"]["post"];
    assert_eq!(
        create["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/NewBook"
    );
    assert_eq!(
        create["responses"]["201"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/Book"
    );
    assert!(create["responses"]["422"].is_object(), "ValidatedJson adds a 422");

    let remove = &spec["paths"]["/books/{id}"]["delete"];
    assert!(remove["responses"]["204"].is_object());
    assert!(remove["responses"]["204"]["content"].is_null());

    assert!(spec["components"]["schemas"]["Book"].is_object());
    assert!(spec["components"]["schemas"]["NewBook"].is_object());
    assert!(spec["components"]["schemas"]["ErrorEnvelope"].is_object());
}

#[tokio::test]
async fn guards_mark_routes_as_requiring_a_bearer_token() {
    let spec = spec().await;
    assert_eq!(spec["components"]["securitySchemes"]["bearerAuth"]["scheme"], "bearer");

    let create = &spec["paths"]["/books"]["post"];
    assert!(create["security"][0]["bearerAuth"].is_array());
    assert!(create["responses"]["401"].is_object());
    assert!(
        spec["paths"]["/books"]["get"]["security"].is_null(),
        "unguarded routes stay open"
    );
}

#[tokio::test]
async fn roles_and_permissions_are_documented_with_a_403() {
    let spec = spec().await;
    let remove = &spec["paths"]["/books/{id}"]["delete"];
    let description = remove["description"].as_str().unwrap();
    assert!(
        description.contains("Requires role admin or librarian; permission books:delete."),
        "{description}"
    );
    assert!(remove["responses"]["403"].is_object());
    assert!(
        spec["paths"]["/books"]["post"]["responses"]["403"].is_null(),
        "no requirement, no 403"
    );
}

#[tokio::test]
async fn types_without_schemas_are_documented_without_them_instead_of_failing() {
    let spec = spec().await;
    let loose = &spec["paths"]["/books/loose"]["post"];
    assert!(loose["requestBody"].is_null(), "no ToSchema, no body schema");
    assert!(loose["responses"]["200"]["content"].is_null());
}

#[tokio::test]
async fn the_document_and_ui_are_served_under_the_global_prefix() {
    let router = App::create(Library)
        .await
        .unwrap()
        .global_prefix("/api")
        .serve_openapi(ApiInfo::new("Library", "1"))
        .into_router();

    let response = router
        .clone()
        .oneshot(Request::builder().uri("/api/openapi.json").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let spec: Value = serde_json::from_slice(&body).unwrap();
    assert!(
        spec["paths"]["/api/books"].is_object(),
        "paths include the prefix: {:?}",
        spec["paths"]
    );

    let page = router
        .oneshot(Request::builder().uri("/api/docs").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(page.status(), 200);
    let html = String::from_utf8(page.into_body().collect().await.unwrap().to_bytes().to_vec()).unwrap();
    assert!(html.contains("/api/openapi.json") && html.contains("swagger-ui"));
}

#[tokio::test]
async fn optional_guards_document_anonymous_access() {
    let spec = spec().await;
    let mine = &spec["paths"]["/books/mine"]["get"];
    assert_eq!(mine["security"][0], serde_json::json!({}), "no token is also fine");
    assert!(mine["security"][1]["bearerAuth"].is_array());
    assert_eq!(
        mine["responses"]["401"]["description"],
        "A bearer token was sent but is invalid or expired"
    );
    // A required guard is unchanged: bearer only, no anonymous entry.
    let create = &spec["paths"]["/books"]["post"];
    assert!(create["security"][0]["bearerAuth"].is_array());
    assert!(create["security"][1].is_null());
}
