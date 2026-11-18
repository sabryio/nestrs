//! Black-box tests: boot the real `AppModule` and drive it over HTTP, in-process.
//! The default token is `dev-token` (see `AppConfig::default`).

use blog_api::AppModule;
use purlin::http::axum::http::StatusCode;
use purlin::openapi::ApiInfo;
use purlin::prelude::App;
use purlin_testing::{TestApp, TestClient};
use serde_json::{Value, json};

const TOKEN: &str = "dev-token";

async fn client() -> TestClient {
    TestApp::start(AppModule).await.client()
}

async fn create_user(client: &TestClient, name: &str, email: &str) -> Value {
    client
        .post("/users")
        .bearer(TOKEN)
        .json(&json!({ "name": name, "email": email }))
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json()
}

#[tokio::test]
async fn health_is_up() {
    let body: Value = client()
        .await
        .get("/health/live")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body, json!({ "status": "ok" }));
}

#[tokio::test]
async fn writes_require_a_valid_token() {
    let client = client().await;
    let user = json!({ "name": "Ada", "email": "ada@example.com" });

    let error: Value = client
        .post("/users")
        .json(&user)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED)
        .json();
    assert_eq!(error["error"]["code"], "unauthorized");

    client
        .post("/users")
        .bearer("wrong")
        .json(&user)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    // Reads stay public.
    client.get("/users").send().await.assert_status(StatusCode::OK);
}

#[tokio::test]
async fn users_can_be_created_and_fetched() {
    let client = client().await;
    let created = create_user(&client, "Ada", "Ada@Example.com").await;
    assert_eq!(created["email"], "ada@example.com");

    let id = created["id"].as_str().unwrap();
    let fetched: Value = client
        .get(&format!("/users/{id}"))
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(fetched, created);

    let all: Value = client.get("/users").send().await.json();
    assert_eq!(all.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn unknown_user_is_a_404_with_the_error_envelope() {
    let error: Value = client()
        .await
        .get("/users/00000000-0000-0000-0000-000000000000")
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND)
        .json();
    assert_eq!(error["error"]["code"], "not_found");
}

#[tokio::test]
async fn validation_failures_list_the_offending_fields() {
    let error: Value = client()
        .await
        .post("/users")
        .bearer(TOKEN)
        .json(&json!({ "name": "", "email": "not-an-email" }))
        .send()
        .await
        .assert_status(StatusCode::UNPROCESSABLE_ENTITY)
        .json();
    assert_eq!(error["error"]["code"], "validation_failed");
    assert!(error["error"]["details"]["name"].is_array());
    assert_eq!(
        error["error"]["details"]["email"][0]["message"],
        "must be a valid email address"
    );
}

#[tokio::test]
async fn malformed_json_is_a_400() {
    client()
        .await
        .post("/users")
        .bearer(TOKEN)
        .header("content-type", "application/json")
        .body("{ nope")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn duplicate_emails_conflict() {
    let client = client().await;
    create_user(&client, "Ada", "ada@example.com").await;
    let error: Value = client
        .post("/users")
        .bearer(TOKEN)
        .json(&json!({ "name": "Other", "email": "ADA@example.com" }))
        .send()
        .await
        .assert_status(StatusCode::CONFLICT)
        .json();
    assert_eq!(error["error"]["code"], "conflict");
}

#[tokio::test]
async fn posts_require_an_existing_author_and_support_the_full_lifecycle() {
    let client = client().await;

    let error: Value = client
        .post("/posts")
        .bearer(TOKEN)
        .json(&json!({ "author_id": "00000000-0000-0000-0000-000000000000", "title": "Hello", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST)
        .json();
    assert!(error["error"]["message"].as_str().unwrap().contains("does not exist"));

    let author = create_user(&client, "Ada", "ada@example.com").await;
    let post: Value = client
        .post("/posts")
        .bearer(TOKEN)
        .json(&json!({ "author_id": author["id"], "title": "  Hello  ", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json();
    assert_eq!(post["title"], "Hello");

    let uri = format!("/posts/{}", post["id"].as_str().unwrap());
    let fetched: Value = client.get(&uri).send().await.assert_status(StatusCode::OK).json();
    assert_eq!(fetched, post);

    client.delete(&uri).send().await.assert_status(StatusCode::UNAUTHORIZED);
    client
        .delete(&uri)
        .bearer(TOKEN)
        .send()
        .await
        .assert_status(StatusCode::NO_CONTENT);
    client.get(&uri).send().await.assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_module_graph_boots_in_dependency_order() {
    let app = App::create(AppModule).await.unwrap();
    let modules = app.modules();
    let position = |name: &str| modules.iter().position(|m| *m == name).unwrap();
    assert!(position("ConfigModule") < position("PersistenceModule"));
    assert!(position("AuthModule") < position("UsersModule"));
    assert!(position("PersistenceModule") < position("UsersModule"));
    assert!(position("UsersModule") < position("PostsModule"));
    assert_eq!(*modules.last().unwrap(), "AppModule");
}

// ------------------------------------------------- pipes, interceptors, scopes

#[tokio::test]
async fn the_page_pipe_paginates_and_rejects_bad_limits() {
    let client = client().await;
    for n in 0..5 {
        create_user(&client, &format!("User {n}"), &format!("user{n}@example.com")).await;
    }

    let all: Value = client.get("/users").send().await.json();
    assert_eq!(all.as_array().unwrap().len(), 5);

    let first_two: Value = client.get("/users?limit=2").send().await.json();
    let rest: Value = client.get("/users?limit=10&offset=2").send().await.json();
    assert_eq!(first_two.as_array().unwrap().len(), 2);
    assert_eq!(rest.as_array().unwrap().len(), 3);
    assert_eq!(first_two[0], all[0]);
    assert_eq!(rest[0], all[2], "offset skips the first two");

    for bad in ["/users?limit=0", "/users?limit=101"] {
        let error: Value = client
            .get(bad)
            .send()
            .await
            .assert_status(StatusCode::BAD_REQUEST)
            .json();
        assert!(error["error"]["message"].as_str().unwrap().contains("limit"), "{error}");
    }
    client
        .get("/users?limit=abc")
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_timing_interceptor_marks_every_response_of_a_controller() {
    let client = client().await;
    let response = client.get("/users").send().await;
    let millis: f64 = response
        .header("x-response-time-ms")
        .expect("interceptor ran")
        .parse()
        .unwrap();
    assert!(millis >= 0.0);
    assert!(client.get("/posts").send().await.header("x-response-time-ms").is_some());
    assert!(
        client
            .get("/health/live")
            .send()
            .await
            .header("x-response-time-ms")
            .is_none(),
        "not intercepted"
    );
}

#[tokio::test]
async fn the_request_scoped_context_is_built_per_request() {
    let client = client().await;
    let register = |name: &str, email: &str| {
        client
            .post("/users")
            .bearer(TOKEN)
            .json(&json!({ "name": name, "email": email }))
    };

    // A caller-supplied id is echoed back...
    let response = register("Ada", "ada@example.com")
        .header("x-request-id", "req-42")
        .send()
        .await;
    assert_eq!(response.header("x-request-id"), Some("req-42"));

    // ...and requests without one each get their own generated id.
    let first = register("Grace", "grace@example.com").send().await;
    let second = register("Edsger", "edsger@example.com").send().await;
    let (a, b) = (
        first.header("x-request-id").unwrap(),
        second.header("x-request-id").unwrap(),
    );
    assert_eq!(a.len(), 36, "a generated UUID: {a}");
    assert_ne!(a, b, "no state leaks from one request to the next");
}

// ----------------------------------------------------------------- OpenAPI

#[tokio::test]
async fn the_api_documents_itself() {
    let app = App::create(AppModule).await.unwrap();
    let spec = serde_json::to_value(app.openapi(&ApiInfo::new("Blog API", "1.0.0"))).unwrap();

    let users = &spec["paths"]["/users"];
    assert_eq!(users["get"]["summary"], "List users.");
    let parameters: Vec<&str> = users["get"]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(parameters, ["limit", "offset"], "the page pipe documents its query");

    assert_eq!(
        users["post"]["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/CreateUserRequest"
    );
    assert!(users["post"]["security"][0]["bearerAuth"].is_array());
    assert!(users["post"]["responses"]["201"].is_object());
    assert!(spec["paths"]["/posts/{id}"]["delete"]["responses"]["204"].is_object());
    assert!(spec["components"]["schemas"]["PostResponse"].is_object());
}

#[tokio::test]
async fn writes_are_rate_limited_per_client_with_retry_after() {
    let client = client().await;
    let write = |n: usize| {
        let client = &client;
        async move {
            client
                .post("/users")
                .bearer(TOKEN)
                .json(&json!({ "name": "Rate", "email": format!("rate{n}@example.com") }))
                .send()
                .await
        }
    };

    for n in 0..60 {
        let response = write(n).await;
        assert_eq!(response.status(), StatusCode::CREATED, "write {n} is within the limit");
        if n == 0 {
            assert_eq!(response.header("ratelimit-limit"), Some("60"));
            assert_eq!(response.header("ratelimit-remaining"), Some("59"));
        }
    }

    let limited = write(60).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    let body: Value = limited.json();
    assert_eq!(body["error"]["code"], "rate_limited");

    // Reads are not covered by the write limit.
    client.get("/users").send().await.assert_status(StatusCode::OK);
}

#[tokio::test]
async fn requests_a_guard_rejects_do_not_use_up_the_write_allowance() {
    let client = client().await;
    for _ in 0..70 {
        client
            .post("/users")
            .json(&json!({ "name": "No token", "email": "x@example.com" }))
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
    create_user(&client, "Ada", "ada@example.com").await;
}

#[tokio::test]
async fn readiness_reports_the_database_check_and_fails_when_it_does() {
    use purlin::prelude::{Arc, Multi};
    use purlin_health::{HealthIndicator, HealthIndicatorPort, Unhealthy};

    let client = client().await;
    let body: Value = client
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::OK)
        .json();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["checks"][0]["name"], "database");
    assert_eq!(body["checks"][0]["status"], "up");

    // Swap the whole set for one with a failing check, the way a test replaces any provider.
    struct Down;
    #[purlin::async_trait]
    impl HealthIndicator for Down {
        fn name(&self) -> &str {
            "database"
        }
        async fn check(&self) -> Result<(), Unhealthy> {
            Err(Unhealthy::new("database unreachable"))
        }
    }
    let failing: HealthIndicatorPort = Arc::new(Down);
    let app = purlin_testing::TestApp::builder(AppModule)
        .override_value(Multi::<dyn HealthIndicator>::new(vec![failing]))
        .start()
        .await;
    let response = app
        .client()
        .get("/health/ready")
        .send()
        .await
        .assert_status(StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json();
    assert_eq!(body["error"]["details"]["checks"][0]["error"], "database unreachable");
    // Liveness does not care.
    app.client()
        .get("/health/live")
        .send()
        .await
        .assert_status(StatusCode::OK);
}

const AUTHOR_TOKEN: &str = "author-token";

#[tokio::test]
async fn roles_and_permissions_decide_who_may_do_what() {
    let client = client().await;
    let user = create_user(&client, "Ada", "ada@example.com").await;
    let post = json!({ "author_id": user["id"], "title": "Hello", "body": "World" });

    // No token: 401 from the guard. Wrong role: 403 from the requirement.
    client
        .post("/posts")
        .json(&post)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let forbidden = client
        .post("/users")
        .bearer(AUTHOR_TOKEN)
        .json(&json!({ "name": "Eve", "email": "eve@example.com" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let body: Value = forbidden.json();
    assert_eq!(body["error"]["code"], "forbidden");
    assert!(
        !body.to_string().contains("admin"),
        "the response does not say which role would work"
    );

    // An author may publish (role author) ...
    let created: Value = client
        .post("/posts")
        .bearer(AUTHOR_TOKEN)
        .json(&post)
        .send()
        .await
        .assert_status(StatusCode::CREATED)
        .json();

    // ... but not delete: that needs the `posts:delete` permission, which only the admin's `*` grants.
    let path = format!("/posts/{}", created["id"].as_str().unwrap());
    client
        .delete(&path)
        .bearer(AUTHOR_TOKEN)
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    client
        .delete(&path)
        .bearer(TOKEN)
        .send()
        .await
        .assert_status(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn the_spec_lists_what_each_route_requires() {
    let app = App::create(AppModule).await.unwrap();
    let spec = serde_json::to_value(app.openapi(&ApiInfo::new("Blog", "1"))).unwrap();
    let delete = &spec["paths"]["/posts/{id}"]["delete"];
    assert!(
        delete["description"]
            .as_str()
            .unwrap()
            .contains("Requires permission posts:delete")
    );
    assert!(delete["responses"]["403"].is_object());
    let create_post = &spec["paths"]["/posts"]["post"];
    assert!(
        create_post["description"]
            .as_str()
            .unwrap()
            .contains("role admin or author")
    );
}

#[tokio::test]
async fn unauthenticated_requests_get_a_bearer_challenge() {
    let client = client().await;
    let response = client
        .post("/users")
        .json(&json!({ "name": "Ada", "email": "ada@example.com" }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    assert_eq!(response.header("www-authenticate"), Some("Bearer"));

    let wrong = client
        .post("/users")
        .bearer("not-a-token")
        .json(&json!({ "name": "Ada", "email": "ada@example.com" }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    let body: Value = wrong.json();
    assert_eq!(body["error"]["message"], "invalid token");
    assert!(!body.to_string().contains("not-a-token"), "the token is never echoed");
}
