use http_body_util::BodyExt;
use purlin_http::axum::Router;
use purlin_http::axum::body::Body;
use purlin_http::axum::http::{HeaderName, HeaderValue, Method, Request, StatusCode, header};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tower::ServiceExt;

/// Sends requests into a router in-process.
#[derive(Clone)]
pub struct TestClient {
    router: Router,
}

impl TestClient {
    pub(crate) fn new(router: Router) -> Self {
        Self { router }
    }

    /// Starts a request with an arbitrary method.
    pub fn request(&self, method: Method, path: &str) -> RequestBuilder {
        RequestBuilder {
            router: self.router.clone(),
            method,
            path: path.to_owned(),
            headers: Vec::new(),
            body: None,
        }
    }

    /// `GET path`.
    pub fn get(&self, path: &str) -> RequestBuilder {
        self.request(Method::GET, path)
    }

    /// `POST path`.
    pub fn post(&self, path: &str) -> RequestBuilder {
        self.request(Method::POST, path)
    }

    /// `PUT path`.
    pub fn put(&self, path: &str) -> RequestBuilder {
        self.request(Method::PUT, path)
    }

    /// `PATCH path`.
    pub fn patch(&self, path: &str) -> RequestBuilder {
        self.request(Method::PATCH, path)
    }

    /// `DELETE path`.
    pub fn delete(&self, path: &str) -> RequestBuilder {
        self.request(Method::DELETE, path)
    }
}

/// A request being built; finish it with [`send`](Self::send).
#[must_use = "a request does nothing until `send().await`"]
pub struct RequestBuilder {
    router: Router,
    method: Method,
    path: String,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Option<Vec<u8>>,
}

impl RequestBuilder {
    /// Adds a header. Panics on an invalid name or value (it is a test bug).
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((
            HeaderName::try_from(name).expect("valid header name"),
            HeaderValue::try_from(value).expect("valid header value"),
        ));
        self
    }

    /// Sends `Authorization: Bearer <token>`.
    pub fn bearer(self, token: &str) -> Self {
        self.header("authorization", &format!("Bearer {token}"))
    }

    /// Sets a JSON body and the matching `Content-Type`.
    pub fn json<T: Serialize + ?Sized>(mut self, body: &T) -> Self {
        self.body = Some(serde_json::to_vec(body).expect("serialisable body"));
        self.header(header::CONTENT_TYPE.as_str(), "application/json")
    }

    /// Sets a raw body (for malformed-input tests).
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// Sends the request through the router and collects the response.
    pub async fn send(self) -> TestResponse {
        let mut request = Request::builder().method(self.method).uri(&self.path);
        for (name, value) in self.headers {
            request = request.header(name, value);
        }
        let body = self.body.map_or_else(Body::empty, Body::from);
        let request = request.body(body).expect("a well-formed request");

        let response = self
            .router
            .oneshot(request)
            .await
            .unwrap_or_else(|never| match never {});
        let (parts, body) = response.into_parts();
        let bytes = body.collect().await.expect("a readable body").to_bytes().to_vec();
        TestResponse {
            status: parts.status,
            headers: parts.headers,
            body: bytes,
        }
    }
}

/// A fully received response.
#[derive(Debug)]
pub struct TestResponse {
    status: StatusCode,
    headers: purlin_http::axum::http::HeaderMap,
    body: Vec<u8>,
}

impl TestResponse {
    /// The status code.
    pub fn status(&self) -> StatusCode {
        self.status
    }

    /// A response header, if present and valid UTF-8.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }

    /// The body as text.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body parsed as JSON into `T`; panics with the body if it does not fit.
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|error| panic!("response body is not the expected JSON ({error}): {}", self.text()))
    }

    /// Fails the test, showing the body, unless the status is `expected`.
    #[track_caller]
    pub fn assert_status(self, expected: StatusCode) -> Self {
        assert_eq!(
            self.status,
            expected,
            "unexpected status; response body: {}",
            self.text()
        );
        self
    }
}
