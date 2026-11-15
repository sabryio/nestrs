/// Joins a controller prefix and a route path into one normalised axum path.
///
/// Leading and trailing slashes on either side are irrelevant, and an empty or
/// `"/"` route maps to the prefix itself (`/users`, not `/users/`).
///
/// ```
/// use nestrs_http::join_path;
/// assert_eq!(join_path("/users", "/"), "/users");
/// assert_eq!(join_path("/users", "/{id}"), "/users/{id}");
/// assert_eq!(join_path("", "/health"), "/health");
/// ```
pub fn join_path(prefix: &str, route: &str) -> String {
    let prefix = prefix.trim_matches('/');
    let route = route.trim_matches('/');
    match (prefix.is_empty(), route.is_empty()) {
        (true, true) => "/".to_owned(),
        (true, false) => format!("/{route}"),
        (false, true) => format!("/{prefix}"),
        (false, false) => format!("/{prefix}/{route}"),
    }
}

#[cfg(test)]
mod tests {
    use super::join_path;

    #[test]
    fn normalises_slashes() {
        assert_eq!(join_path("users", "id"), "/users/id");
        assert_eq!(join_path("/users/", "/{id}/"), "/users/{id}");
        assert_eq!(join_path("/", "/"), "/");
        assert_eq!(join_path("/a/b", ""), "/a/b");
    }
}
