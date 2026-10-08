//! Server-side rendering.

#[cfg(feature = "ssr")]
use std::fmt;
use std::future::Future;
#[cfg(feature = "ssr")]
use std::path::PathBuf;
#[cfg(feature = "ssr")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "ssr")]
use std::sync::{Arc, OnceLock};
#[cfg(feature = "ssr")]
use std::time::Duration;

use futures_util::future::BoxFuture;

#[cfg(feature = "ssr")]
use crate::files;
use crate::page::Page;
use crate::request::Request;

/// Server-side rendered HTML for a page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// The `<head>` elements, such as the `<title>` set with `<Head>`.
    pub head: String,
    /// The app element with its rendered HTML, and the page data `<script>`.
    pub body: String,
}

/// Renders pages on the server.
///
/// Returning `None` falls back to client-side rendering, so a gateway that
/// can't reach its renderer should log and return `None` rather than fail.
pub trait Gateway: Send + Sync + 'static {
    /// Render a page.
    fn dispatch(&self, page: &Page, request: &Request) -> impl Future<Output = Option<Rendered>> + Send;
}

/// An object-safe [`Gateway`].
pub(crate) trait DynGateway: Send + Sync {
    fn dispatch<'a>(&'a self, page: &'a Page, request: &'a Request) -> BoxFuture<'a, Option<Rendered>>;
}

impl<G: Gateway> DynGateway for G {
    fn dispatch<'a>(&'a self, page: &'a Page, request: &'a Request) -> BoxFuture<'a, Option<Rendered>> {
        Box::pin(Gateway::dispatch(self, page, request))
    }
}

/// Renders pages through an Inertia SSR server over HTTP.
///
/// While the Vite dev server is running (its hot file exists), pages are
/// rendered by the `@inertiajs/vite` plugin's `/__inertia_ssr` endpoint.
/// Otherwise they're sent to the production SSR server's `/render` endpoint,
/// started with `node bootstrap/ssr/app.js`.
#[cfg(feature = "ssr")]
#[derive(Clone)]
pub struct HttpGateway {
    url: String,
    hot_file: Option<PathBuf>,
    bundle: Option<PathBuf>,
    watch: bool,
    except: Vec<String>,
    enabled: bool,
    disable_when: Option<DisableWhen>,
    on_failure: Option<OnFailure>,
    client: reqwest::Client,
}

#[cfg(feature = "ssr")]
type DisableWhen = Arc<dyn Fn(&Request) -> bool + Send + Sync>;

#[cfg(feature = "ssr")]
type OnFailure = Arc<dyn Fn(&SsrFailure, &Page) + Send + Sync>;

#[cfg(feature = "ssr")]
impl fmt::Debug for HttpGateway {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpGateway")
            .field("url", &self.url)
            .field("hot_file", &self.hot_file)
            .field("bundle", &self.bundle)
            .field("watch", &self.watch)
            .field("except", &self.except)
            .field("enabled", &self.enabled)
            .field("disable_when", &self.disable_when.is_some())
            .field("on_failure", &self.on_failure.is_some())
            .field("client", &self.client)
            .finish()
    }
}

#[cfg(feature = "ssr")]
impl Default for HttpGateway {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "ssr")]
impl HttpGateway {
    /// A gateway for the SSR server at `http://127.0.0.1:13714`.
    pub fn new() -> Self {
        Self {
            url: "http://127.0.0.1:13714".to_owned(),
            hot_file: None,
            bundle: None,
            watch: true,
            except: Vec::new(),
            enabled: true,
            disable_when: None,
            on_failure: None,
            client: default_client(),
        }
    }

    /// Set the production SSR server URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Render through the Vite dev server while this hot file exists. The
    /// file holds the dev server's URL.
    pub fn hot_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.hot_file = Some(path.into());
        self
    }

    /// Only use the production SSR server when this bundle has been built.
    pub fn bundle(mut self, path: impl Into<PathBuf>) -> Self {
        self.bundle = Some(path.into());
        self
    }

    /// Set whether the hot file and the bundle are checked on every render.
    ///
    /// On by default, so starting the dev server or building the bundle takes
    /// effect without a restart. Turn it off in production, where neither
    /// changes without a deploy, to check each once.
    pub fn watch(mut self, watch: bool) -> Self {
        self.watch = watch;
        self
    }

    /// Never server-side render these paths. A trailing `*` matches any suffix.
    pub fn except<I, P>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: Into<String>,
    {
        self.except.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Enable or disable server-side rendering.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Don't server-side render the requests this returns `true` for.
    ///
    /// It's called on every render that [`enabled`](Self::enabled) allows.
    pub fn disable_when(mut self, condition: impl Fn(&Request) -> bool + Send + Sync + 'static) -> Self {
        self.disable_when = Some(Arc::new(condition));
        self
    }

    /// Set the request timeout.
    ///
    /// This builds a new client, replacing one set with [`client`](Self::client).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.client = client(timeout);
        self
    }

    /// Send requests to the SSR server, health checks included, with this
    /// client, to set headers, a proxy or a timeout.
    ///
    /// Only the client's own timeout applies, so give it one: without it, a
    /// hung SSR server holds up the response indefinitely.
    pub fn client(mut self, client: reqwest::Client) -> Self {
        self.client = client;
        self
    }

    /// Call this when rendering fails, before falling back to client-side
    /// rendering. It's for reporting, like Laravel's `SsrRenderFailed` event:
    /// failures are logged either way.
    pub fn on_failure(mut self, callback: impl Fn(&SsrFailure, &Page) + Send + Sync + 'static) -> Self {
        self.on_failure = Some(Arc::new(callback));
        self
    }

    /// Whether the production SSR server is up.
    pub async fn is_healthy(&self) -> bool {
        self.client
            .get(format!("{}/health", self.url))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    /// The URL to render with, or `None` when SSR is unavailable.
    fn endpoint(&self, request: &Request) -> Option<String> {
        if !self.enabled
            || self.disable_when.as_ref().is_some_and(|disabled| disabled(request))
            || self.is_excluded(request.path())
        {
            return None;
        }

        if let Some(hot) = self.hot_file.as_ref().and_then(|path| files::read(path, self.watch)) {
            return Some(format!("{}/__inertia_ssr", hot.trim().trim_end_matches('/')));
        }

        if self
            .bundle
            .as_ref()
            .is_some_and(|bundle| !files::exists(bundle, self.watch))
        {
            return None;
        }

        Some(format!("{}/render", self.url))
    }

    fn is_excluded(&self, path: &str) -> bool {
        let path = path.trim_start_matches('/');

        self.except.iter().any(|pattern| {
            let pattern = pattern.trim_start_matches('/');

            match pattern.strip_suffix('*') {
                Some(prefix) => path.starts_with(prefix),
                None => path == pattern,
            }
        })
    }

    fn report(&self, failure: &SsrFailure, page: &Page) {
        if let Some(on_failure) = &self.on_failure {
            on_failure(failure, page);
        }
    }
}

#[cfg(feature = "ssr")]
impl Gateway for HttpGateway {
    async fn dispatch(&self, page: &Page, request: &Request) -> Option<Rendered> {
        #[derive(serde::Deserialize)]
        struct Response {
            #[serde(default)]
            head: Vec<String>,
            #[serde(default)]
            body: String,
        }

        let url = self.endpoint(request)?;

        let response = match self.client.post(&url).json(page).send().await {
            Ok(response) => response,
            Err(error) => {
                // Warn once: an SSR server that isn't running is a setup
                // issue, not something to repeat on every request.
                if !WARNED_UNREACHABLE.swap(true, Ordering::Relaxed) {
                    tracing::warn!(%url, %error, "the Inertia SSR server is unreachable; rendering on the client");
                } else {
                    tracing::debug!(%url, %error, "the Inertia SSR server is unreachable");
                }
                self.report(&SsrFailure::new(error.to_string(), SsrErrorType::Connection), page);
                return None;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let failure = SsrFailure::from_body(&response.text().await.unwrap_or_default());
            tracing::error!(
                %url,
                %status,
                component = %page.component,
                error = %failure.error,
                kind = %failure.kind,
                hint = failure.hint.as_deref(),
                source_location = failure.source_location.as_deref(),
                "Inertia SSR failed; rendering on the client"
            );
            self.report(&failure, page);
            return None;
        }

        // The Vite plugin responds with `null` while it warms up.
        match response.json::<Option<Response>>().await {
            Ok(rendered) => rendered.map(|Response { head, body }| Rendered {
                head: head.join("\n"),
                body,
            }),
            Err(error) => {
                tracing::error!(%url, %error, "invalid Inertia SSR response; rendering on the client");
                None
            }
        }
    }
}

/// Why the SSR server failed to render a page, as reported to
/// [`HttpGateway::on_failure`].
#[cfg(feature = "ssr")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsrFailure {
    /// The error message.
    pub error: String,
    /// What kind of error it was.
    pub kind: SsrErrorType,
    /// How to fix the error.
    pub hint: Option<String>,
    /// The browser API that was accessed, for [`SsrErrorType::BrowserApi`].
    pub browser_api: Option<String>,
    /// The stack trace.
    pub stack: Option<String>,
    /// Where the error occurred, as `file:line:column`.
    pub source_location: Option<String>,
}

#[cfg(feature = "ssr")]
impl SsrFailure {
    fn new(error: impl Into<String>, kind: SsrErrorType) -> Self {
        Self {
            error: error.into(),
            kind,
            hint: None,
            browser_api: None,
            stack: None,
            source_location: None,
        }
    }

    /// Parse the SSR server's error response.
    fn from_body(body: &str) -> Self {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Body {
            error: Option<String>,
            #[serde(rename = "type")]
            kind: Option<String>,
            hint: Option<String>,
            browser_api: Option<String>,
            stack: Option<String>,
            source_location: Option<String>,
        }

        let Ok(parsed) = serde_json::from_str::<Body>(body) else {
            // A proxy in front of the SSR server may answer in plain text.
            let text = body.trim();
            return Self::new(
                if text.is_empty() { UNKNOWN_ERROR } else { text },
                SsrErrorType::Unknown,
            );
        };

        Self {
            error: parsed.error.unwrap_or_else(|| UNKNOWN_ERROR.to_owned()),
            kind: parsed
                .kind
                .as_deref()
                .map_or(SsrErrorType::Unknown, SsrErrorType::parse),
            hint: parsed.hint,
            browser_api: parsed.browser_api,
            stack: parsed.stack,
            source_location: parsed.source_location,
        }
    }
}

#[cfg(feature = "ssr")]
const UNKNOWN_ERROR: &str = "Unknown SSR error";

/// The kind of an [`SsrFailure`].
#[cfg(feature = "ssr")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SsrErrorType {
    /// A browser API, such as `window`, was used on the server.
    BrowserApi,
    /// The page component couldn't be found.
    ComponentResolution,
    /// The component threw while rendering.
    Render,
    /// The SSR server couldn't be reached.
    Connection,
    /// Any other error.
    Unknown,
}

#[cfg(feature = "ssr")]
impl SsrErrorType {
    /// The type named in an SSR server's response, such as `browser-api`, or
    /// [`Unknown`](Self::Unknown) for a name it doesn't recognize.
    pub fn parse(name: &str) -> Self {
        match name {
            "browser-api" => Self::BrowserApi,
            "component-resolution" => Self::ComponentResolution,
            "render" => Self::Render,
            "connection" => Self::Connection,
            _ => Self::Unknown,
        }
    }

    /// The type's name in an SSR server's response.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BrowserApi => "browser-api",
            Self::ComponentResolution => "component-resolution",
            Self::Render => "render",
            Self::Connection => "connection",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(feature = "ssr")]
impl fmt::Display for SsrErrorType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(feature = "ssr")]
static WARNED_UNREACHABLE: AtomicBool = AtomicBool::new(false);

/// The client shared by gateways with the default timeout. Clients pool
/// connections, so building one per gateway would waste them.
#[cfg(feature = "ssr")]
fn default_client() -> reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

    CLIENT.get_or_init(|| client(Duration::from_secs(5))).clone()
}

#[cfg(feature = "ssr")]
fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .expect("the SSR HTTP client should build")
}

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    #[test]
    fn parses_the_ssr_server_error() {
        let failure = SsrFailure::from_body(
            r#"{
                "error": "window is not defined",
                "type": "browser-api",
                "hint": "Move it into onMounted",
                "browserApi": "window",
                "stack": "at Home",
                "sourceLocation": "Home.vue:3:7"
            }"#,
        );

        assert_eq!(
            failure,
            SsrFailure {
                error: "window is not defined".to_owned(),
                kind: SsrErrorType::BrowserApi,
                hint: Some("Move it into onMounted".to_owned()),
                browser_api: Some("window".to_owned()),
                stack: Some("at Home".to_owned()),
                source_location: Some("Home.vue:3:7".to_owned()),
            }
        );
    }

    #[test]
    fn unrecognized_errors_are_unknown() {
        let unknown = SsrFailure::new(UNKNOWN_ERROR, SsrErrorType::Unknown);

        assert_eq!(SsrFailure::from_body(r#"{"type": "nope"}"#), unknown);
        assert_eq!(SsrFailure::from_body(r#"{"type": null}"#), unknown);
        assert_eq!(SsrFailure::from_body(""), unknown);
        assert_eq!(
            SsrFailure::from_body("Bad Gateway\n"),
            SsrFailure::new("Bad Gateway", SsrErrorType::Unknown)
        );
    }

    #[test]
    fn error_types_round_trip_their_names() {
        for kind in [
            SsrErrorType::BrowserApi,
            SsrErrorType::ComponentResolution,
            SsrErrorType::Render,
            SsrErrorType::Connection,
            SsrErrorType::Unknown,
        ] {
            assert_eq!(SsrErrorType::parse(kind.as_str()), kind);
        }
    }
}
