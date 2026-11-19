//! `POST /greetings/{name}` and `GET /greetings`.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};

use super::Controller;
use crate::domain::{GreetError, GreetingService};

/// Maps the greeting use case onto HTTP.
pub struct GreetingController {
    service: Arc<GreetingService>,
}

#[fx::component]
impl GreetingController {
    /// Constructor.
    #[fx::constructor]
    pub fn new(service: Arc<GreetingService>) -> Self {
        Self { service }
    }
}

#[fx::bind(group)]
impl Controller for GreetingController {
    fn routes(&self) -> Router {
        Router::new()
            .route("/greetings", get(history))
            .route("/greetings/{name}", post(greet))
            .with_state(self.service.clone())
    }
}

async fn greet(State(service): State<Arc<GreetingService>>, Path(name): Path<String>) -> Response {
    match service.greet(&name) {
        Ok(greeting) => (StatusCode::CREATED, Json(greeting)).into_response(),
        Err(error @ GreetError::EmptyName) => (StatusCode::UNPROCESSABLE_ENTITY, error.to_string()).into_response(),
    }
}

async fn history(State(service): State<Arc<GreetingService>>) -> impl IntoResponse {
    Json(service.history())
}
