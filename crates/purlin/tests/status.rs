//! `#[status]` and `#[response]`: declared success status and documented errors.

use purlin::http::axum::body::Body;
use purlin::http::axum::http::Request;
use purlin::openapi::{ApiInfo, ToSchema};
use purlin::prelude::*;
use serde::Serialize;
use serde_json::Value;
use tower::ServiceExt;

#[derive(Serialize, ToSchema)]
struct Note {
    text: String,
}

#[injectable]
struct NotesController;

#[controller("/notes")]
impl NotesController {
    /// Creates a note. Returns plain JSON; the declared status makes it a 201.
    #[post]
    #[status(201)]
    async fn create(&self) -> Json<Note> {
        Json(Note { text: "hi".into() })
    }

    #[delete("/{id}")]
    #[status(204)]
    #[response(404, "No such note")]
    #[response(409, "The note is locked")]
    async fn remove(&self, Path(id): Path<u32>) -> Result<(), HttpError> {
        match id {
            0 => Err(HttpError::not_found("no such note")),
            _ => Ok(()),
        }
    }

    /// An explicit status chosen by the handler is never overridden.
    #[post("/explicit")]
    #[status(201)]
    async fn explicit(&self) -> StatusCode {
        StatusCode::ACCEPTED
    }
}

#[module(controllers = [NotesController])]
struct NotesModule;

async fn call(method: &str, uri: &str) -> u16 {
    let router = App::create(NotesModule).await.unwrap().into_router();
    router
        .oneshot(Request::builder().method(method).uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
async fn a_default_200_becomes_the_declared_status() {
    assert_eq!(call("POST", "/notes").await, 201);
    assert_eq!(call("DELETE", "/notes/5").await, 204);
}

#[tokio::test]
async fn errors_and_explicit_statuses_are_left_alone() {
    assert_eq!(call("DELETE", "/notes/0").await, 404);
    assert_eq!(call("POST", "/notes/explicit").await, 202);
}

#[tokio::test]
async fn the_declared_status_and_responses_are_documented() {
    let app = App::create(NotesModule).await.unwrap();
    let spec: Value = serde_json::to_value(app.openapi(&ApiInfo::new("Notes", "1"))).unwrap();

    let create = &spec["paths"]["/notes"]["post"]["responses"];
    assert!(create["201"].is_object() && create["200"].is_null(), "{create}");

    let remove = &spec["paths"]["/notes/{id}"]["delete"]["responses"];
    assert!(remove["204"].is_object(), "{remove}");
    assert_eq!(remove["404"]["description"], "No such note", "{remove}");
    assert_eq!(remove["409"]["description"], "The note is locked", "{remove}");
}
