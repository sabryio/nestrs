use purlin_http::Interceptor;
use purlin_http::axum::extract::Request;
use purlin_http::axum::http::{HeaderValue, header};
use purlin_http::axum::middleware::Next;
use purlin_http::axum::response::Response;

/// The language chosen for a request. Take it as a handler parameter once
/// [`LocaleNegotiator`] is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale(String);

impl Locale {
    /// The language tag, in the spelling you configured (`"en"`, `"ar"`, `"pt-BR"`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

extension_extractor!(Locale, "LocaleNegotiator");

/// Picks the best supported language from `Accept-Language`.
///
/// Honours quality values (`ar;q=0.9, en;q=0.8`), matches a full tag first (`pt-BR`)
/// and then its base language (`pt-PT` gets `pt` if that is all you support), and falls
/// back to the default for anything else, including `*`, a missing header and
/// malformed input (negotiation never fails a request). The response carries
/// `Content-Language` and `Vary: Accept-Language`, so caches keep one copy per language.
#[derive(Debug, Clone)]
pub struct LocaleNegotiator {
    supported: Vec<String>,
    default: String,
}

impl LocaleNegotiator {
    /// Supports `supported`; `default` is used when nothing matches and must be one of them.
    ///
    /// # Panics
    /// If `default` is not in `supported`.
    pub fn new(supported: impl IntoIterator<Item = &'static str>, default: &'static str) -> Self {
        let supported: Vec<String> = supported.into_iter().map(str::to_owned).collect();
        assert!(
            supported.iter().any(|tag| tag.eq_ignore_ascii_case(default)),
            "the default locale must be one of the supported ones"
        );
        Self {
            supported,
            default: default.to_owned(),
        }
    }

    /// The best match for an `Accept-Language` value.
    pub fn negotiate(&self, accept_language: Option<&str>) -> Locale {
        let mut wanted: Vec<(&str, f32)> = accept_language
            .unwrap_or("")
            .split(',')
            .filter_map(|item| {
                let mut pieces = item.split(';');
                let tag = pieces.next()?.trim();
                if tag.is_empty() {
                    return None;
                }
                let quality = pieces
                    .find_map(|p| p.trim().strip_prefix("q="))
                    .map_or(Some(1.0), |q| q.trim().parse::<f32>().ok())?;
                (quality > 0.0).then_some((tag, quality))
            })
            .collect();
        wanted.sort_by(|a, b| b.1.total_cmp(&a.1));

        let find = |tag: &str| self.supported.iter().find(|s| s.eq_ignore_ascii_case(tag)).cloned();
        let chosen = wanted
            .iter()
            .find_map(|(tag, _)| find(tag).or_else(|| tag.split('-').next().and_then(find)));
        Locale(chosen.unwrap_or_else(|| self.default.clone()))
    }
}

impl Interceptor for LocaleNegotiator {
    async fn intercept(&self, mut request: Request, next: Next) -> Response {
        let header_value = request
            .headers()
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok());
        let locale = self.negotiate(header_value);
        let language = HeaderValue::from_str(locale.as_str()).ok();
        request.extensions_mut().insert(locale);

        let mut response = next.run(request).await;
        let headers = response.headers_mut();
        if let Some(language) = language
            && !headers.contains_key(header::CONTENT_LANGUAGE)
        {
            headers.insert(header::CONTENT_LANGUAGE, language);
        }
        headers.append(header::VARY, HeaderValue::from_static("accept-language"));
        response
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn negotiator() -> LocaleNegotiator {
        LocaleNegotiator::new(["en", "ar", "pt-BR"], "en")
    }

    #[test]
    fn honours_quality_and_falls_back_to_the_base_language() {
        let n = negotiator();
        assert_eq!(n.negotiate(Some("ar;q=0.9, en;q=0.8")).as_str(), "ar");
        assert_eq!(n.negotiate(Some("en;q=0.5, ar")).as_str(), "ar");
        assert_eq!(n.negotiate(Some("ar-EG,en;q=0.5")).as_str(), "ar");
        assert_eq!(n.negotiate(Some("pt-br")).as_str(), "pt-BR");
    }

    #[test]
    fn anything_unusable_gets_the_default() {
        let n = negotiator();
        for header in [
            None,
            Some(""),
            Some("*"),
            Some("fr, de;q=0.7"),
            Some("ar;q=abc"),
            Some("ar;q=0"),
            Some(";;,"),
        ] {
            assert_eq!(n.negotiate(header).as_str(), "en", "{header:?}");
        }
    }

    #[tokio::test]
    async fn sets_content_language_and_vary() {
        use crate::testing::{get, router_with};
        let response = get(&router_with(negotiator()), "/", &[("accept-language", "ar")]).await;
        assert_eq!(response.headers()["content-language"], "ar");
        assert_eq!(response.headers()["vary"], "accept-language");
    }
}
