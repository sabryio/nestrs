//! A complete nestrs application in one file: a todo list over HTTP.
//!
//! ```text
//! cargo run -p todo-minimal
//! curl -X POST localhost:3000/todos -H 'content-type: application/json' -d '{"title":"learn nestrs"}'
//! curl localhost:3000/todos
//! curl -X POST localhost:3000/todos/1/complete
//! curl localhost:3000/summary
//! ```

use std::sync::Mutex;

use nestrs::prelude::*;
use serde::{Deserialize, Serialize};

// ── 1. Data ─────────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
struct Todo {
    id: u32,
    title: String,
    done: bool,
}

#[derive(Deserialize)]
struct NewTodo {
    title: String,
}

// ── 2. A port: what the rest of the app needs, not how it is done ───────────
// `#[port]` also generates the alias `TodoStorePort` (an injectable handle).

#[port]
trait TodoStore {
    async fn all(&self) -> Vec<Todo>;
    async fn add(&self, title: String) -> Todo;
    async fn complete(&self, id: u32) -> Option<Todo>;
}

// ── 3. An adapter: one implementation of the port ───────────────────────────
// `provides = TodoStorePort` says "inject me wherever the port is asked for".

#[injectable(provides = TodoStorePort)]
struct MemoryStore {
    #[inject(default)] // not a dependency: start empty
    todos: Mutex<Vec<Todo>>,
}

#[async_trait]
impl TodoStore for MemoryStore {
    async fn all(&self) -> Vec<Todo> {
        self.todos.lock().unwrap().clone()
    }

    async fn add(&self, title: String) -> Todo {
        let mut todos = self.todos.lock().unwrap();
        let todo = Todo {
            id: todos.len() as u32 + 1,
            title,
            done: false,
        };
        todos.push(todo.clone());
        todo
    }

    async fn complete(&self, id: u32) -> Option<Todo> {
        let mut todos = self.todos.lock().unwrap();
        let todo = todos.iter_mut().find(|todo| todo.id == id)?;
        todo.done = true;
        Some(todo.clone())
    }
}

// ── 4. A controller: routes are plain async methods ─────────────────────────
// The field is injected by its type. Nothing here knows about `MemoryStore`.

#[injectable]
struct TodoController {
    store: TodoStorePort,
}

#[controller("/todos")]
impl TodoController {
    #[get]
    async fn list(&self) -> Json<Vec<Todo>> {
        Json(self.store.all().await)
    }

    #[post]
    async fn create(&self, Json(body): Json<NewTodo>) -> (StatusCode, Json<Todo>) {
        (StatusCode::CREATED, Json(self.store.add(body.title).await))
    }

    #[post("/{id}/complete")]
    async fn complete(&self, Path(id): Path<u32>) -> Result<Json<Todo>, HttpError> {
        match self.store.complete(id).await {
            Some(todo) => Ok(Json(todo)),
            None => Err(HttpError::not_found(format!("todo {id} does not exist"))),
        }
    }
}

// ── 5. `Inject<T>`: take a service for one handler instead of a field ──────
// A controller field suits a service every route uses. `Inject<T>` suits one handler
// that needs something extra, and the only way to get a request-scoped provider.
// It is nestrs's answer to axum's `State`: resolved by type, and checked at startup
// (a missing or invisible `T` is a startup error, not a 500).

#[derive(Serialize)]
struct Summary {
    total: usize,
    done: usize,
}

#[injectable]
struct SummaryController; // no fields: everything arrives per handler

#[controller("/summary")]
impl SummaryController {
    /// Counts the todos. `dyn TodoStore` is the port, so a fake store is used here too.
    #[get]
    async fn summary(&self, Inject(store): Inject<dyn TodoStore>) -> Json<Summary> {
        let todos = store.all().await;
        Json(Summary {
            total: todos.len(),
            done: todos.iter().filter(|todo| todo.done).count(),
        })
    }
}

// ── 6. A module wires it together; the graph is checked at startup ──────────

#[module(providers = [MemoryStore], controllers = [TodoController, SummaryController])]
struct AppModule;

#[tokio::main]
async fn main() -> Result<(), AppError> {
    App::create(AppModule).await?.listen("127.0.0.1:3000").await
}

// ── 7. Tests boot the real module in-process; no ports, no sleeps ───────────

#[cfg(test)]
mod tests {
    use nestrs_testing::TestApp;
    use serde_json::{Value, json};

    use super::*;

    #[tokio::test]
    async fn create_complete_and_list() {
        let app = TestApp::start(AppModule).await;
        let client = app.client();

        client
            .post("/todos")
            .json(&json!({ "title": "learn nestrs" }))
            .send()
            .await
            .assert_status(StatusCode::CREATED);
        client
            .post("/todos/1/complete")
            .send()
            .await
            .assert_status(StatusCode::OK);

        let todos: Value = client.get("/todos").send().await.json();
        assert_eq!(todos[0]["title"], "learn nestrs");
        assert_eq!(todos[0]["done"], true);
    }

    #[tokio::test]
    async fn inject_resolves_the_same_store_the_controller_field_uses() {
        let app = TestApp::start(AppModule).await;
        let client = app.client();
        for title in ["a", "b", "c"] {
            client.post("/todos").json(&json!({ "title": title })).send().await;
        }
        client.post("/todos/2/complete").send().await;

        let summary: Value = client.get("/summary").send().await.assert_status(StatusCode::OK).json();
        assert_eq!(summary, json!({ "total": 3, "done": 1 }));
    }

    #[tokio::test]
    async fn completing_an_unknown_todo_is_a_404() {
        let app = TestApp::start(AppModule).await;
        app.client()
            .post("/todos/9/complete")
            .send()
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    /// Swap the adapter for a fake without touching the module.
    struct Empty;

    #[async_trait]
    impl TodoStore for Empty {
        async fn all(&self) -> Vec<Todo> {
            vec![Todo {
                id: 7,
                title: "from the fake".into(),
                done: false,
            }]
        }
        async fn add(&self, _: String) -> Todo {
            unreachable!()
        }
        async fn complete(&self, _: u32) -> Option<Todo> {
            None
        }
    }

    #[tokio::test]
    async fn a_port_can_be_mocked() {
        let app = TestApp::builder(AppModule)
            .mock::<TodoStorePort>(Arc::new(Empty))
            .start()
            .await;
        let todos: Value = app.client().get("/todos").send().await.json();
        assert_eq!(todos[0]["title"], "from the fake");
    }
}
