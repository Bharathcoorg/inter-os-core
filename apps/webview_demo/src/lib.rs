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
        webview.load_html(DEMO_HTML, Some("interos://demo/index.html"))?;

        Ok(Self { webview })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_app_initializes_and_loads_demo_html() {
        let app = WebViewDemoApp::new().expect("demo app init");
        assert_eq!(app.webview.title(), "InterWebView Demo");
        assert_eq!(app.webview.current_url(), "interos://demo/index.html");
    }
}
