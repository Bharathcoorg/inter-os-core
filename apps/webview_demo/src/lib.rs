//! Demonstrator library for InterWebView embedding platform.

#![no_std]

pub use interos_std as std;
use interos_std::webview::{InterWebView, WebViewConfig, WebViewSecurityPolicy};

pub const DEMO_HTML: &str = r#"
<!DOCTYPE html>
<html>
<head><title>InterWebView Demo</title></head>
<body>
  <h1>InterWebView Platform</h1>
  <p>Embedded Webview running in Ring 3.</p>
  <p>Zero ambient authority with capability-gated native bridge.</p>
</body>
</html>
"#;

pub const FETCHED_DEMO_BYTES: &[u8] = b"<!DOCTYPE html><html><head><title>Fetched Article</title></head><body><h1>Host Pipeline</h1><p>Real network content delivered via load_url_content feeder.</p></body></html>";

pub struct WebViewDemoApp {
    pub webview: InterWebView,
}

impl WebViewDemoApp {
    pub fn new() -> Result<Self, &'static str> {
        let mut config = WebViewConfig::default_portrait();
        config.policy = WebViewSecurityPolicy {
            javascript_enabled: true,
            js_bridge_enabled: false, // Default secure: no bridge without grant
            network_access_enabled: false,
            js_step_budget: 2000,
        };

        let mut webview = InterWebView::create_webview(config)?;
        // Use load_html for local rich demonstrator content
        webview.load_html(DEMO_HTML, Some("interos://demo/index.html"))?;

        Ok(Self { webview })
    }

    /// Feeds real host-fetched network responses into the embedded webview,
    /// demonstrating truthful lifecycle progression without fake HTTP 200 responses.
    pub fn feed_fetched_content(
        &mut self,
        url: &str,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<(), &'static str> {
        self.webview.load_url_content(url, status, content_type, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interos_std::webview::{WebViewEvent, WebViewState};

    #[test]
    fn demo_app_initializes_and_loads_demo_html() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        assert_eq!(app.webview.title(), "InterWebView Demo");
        assert_eq!(app.webview.current_url(), "interos://demo/index.html");
        assert_eq!(app.webview.state(), WebViewState::Loaded);

        // Drain initial load events
        let mut got_finished = false;
        while let Some(evt) = app.webview.poll_event() {
            if let WebViewEvent::PageFinished(.., status) = evt {
                assert_eq!(status, 200);
                got_finished = true;
            }
        }
        assert!(got_finished);
    }

    #[test]
    fn demo_app_feeds_fetched_content_truthfully() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        // Clear initial events
        while app.webview.poll_event().is_some() {}

        let res = app.feed_fetched_content(
            "https://interos.org/news",
            200,
            "text/html",
            FETCHED_DEMO_BYTES,
        );
        assert!(res.is_ok());
        assert_eq!(app.webview.title(), "Fetched Article");
        assert_eq!(app.webview.current_url(), "https://interos.org/news");
        assert_eq!(app.webview.state(), WebViewState::Loaded);

        let mut got_title = false;
        let mut got_finished = false;
        while let Some(evt) = app.webview.poll_event() {
            match evt {
                WebViewEvent::TitleChanged(..) => got_title = true,
                WebViewEvent::PageFinished(url_buf, len, status) => {
                    assert_eq!(status, 200);
                    let url_str = core::str::from_utf8(&url_buf[..len]).unwrap();
                    assert_eq!(url_str, "https://interos.org/news");
                    got_finished = true;
                }
                _ => {}
            }
        }
        assert!(got_title);
        assert!(got_finished);
    }

    #[test]
    fn demo_app_refuses_fake_http_load_url() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        // Policy has network_access_enabled = false by default
        let res = app.webview.load_url("https://unreachable.org");
        assert_eq!(res, Err("NetworkAccessDenied: network requests disabled by policy"));
        assert_eq!(app.webview.state(), WebViewState::Error);
    }
}
