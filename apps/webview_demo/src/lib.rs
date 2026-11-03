// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Demonstrator library for InterWebView embedding platform.

#![no_std]

pub use interos_std as std;
use interos_std::design::{back_button, monochrome, viewport};
use interos_std::webview::{InterWebView, WebViewConfig, WebViewSecurityPolicy};

/// The theme the back control is cut from: the dark half of the platform's
/// monochrome ladder, because this app's band is dark.
///
/// WHY not the light one the rest of the fleet hands `pixel_for_theme`: the chip
/// takes its ring, fill and ink from the theme it is given, and the light roles
/// put a `#EFEFE9` plate with a `#0A0A0A` outline on this app's `#121212` band --
/// a ring one hundredth of a step off the band it stands on, so the control
/// reads as a lit panel rather than a button. The dark variant trades paper for
/// ink: the plate lifts to `#232323` (1.19:1 over the band) and the ring and the
/// chevron carry `#FAFAFA`, 17.95:1 against the band and 15.06:1 against the
/// plate. All three are roles of one shipped theme; none is a colour here.
pub const THEME: std::design::Theme = monochrome::THEME_DARK;

/// The colour the page is set on, and the colour its chrome continues into.
///
/// WHY repeated here: `InterWebView` paints its own container colour for every
/// row inside its band and answers nothing at all outside it, so the rows above
/// and below can only be set by the app that owns the surface. Anything other
/// than the page's own colour would read as a second panel behind the document.
pub const PAGE_BG: u32 = 0xFF121212;

/// The address of the demo's first screen, and the only one the document can
/// be at without having been navigated away from.
pub const DEMO_URL: &str = "interos://demo/index.html";

/// The column the document's band starts on: the shared back control's right edge.
///
/// WHY the band moves rather than the text: `InterWebView` sets its own runs a
/// step inside its band and the band is the one edge this app owns, so the
/// control keeps the header's first 72 columns and the title lands on the
/// platform's header column. A band that starts past the control's box cannot
/// put ink under it whatever the controller's inset turns out to be.
pub const DOC_X: usize = back_button::X + back_button::SIZE;

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
        config.x = DOC_X as u32;
        config.w = (viewport::LOGICAL_WIDTH - DOC_X) as u32;
        config.policy = WebViewSecurityPolicy {
            javascript_enabled: true,
            js_bridge_enabled: false, // Default secure: no bridge without grant
            network_access_enabled: false,
            js_step_budget: 2000,
        };

        let mut webview = InterWebView::create_webview(config)?;
        // Use load_html for local rich demonstrator content
        webview.load_html(DEMO_HTML, Some(DEMO_URL))?;

        Ok(Self { webview })
    }

    /// Whether the document on the panel is the one the demo opened with.
    pub fn at_first_screen(&self) -> bool {
        self.webview.current_url() == DEMO_URL
    }

    /// Unwinds a document the reader reached through the feeder back to the
    /// demo's own page, and puts the page back at its first line.
    pub fn show_first_screen(&mut self) -> Result<(), &'static str> {
        let res = self.webview.load_html(DEMO_HTML, Some(DEMO_URL));
        if res.is_ok() {
            self.webview.scroll_mut().offset = 0;
        }
        res
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

    /// The document's band starts clear of the control painted over the header.
    ///
    /// WHY the band edge and not the glyph: the app cannot move a run the
    /// controller sets inside its own band, so the one position it can promise
    /// is that the band itself begins past the control's box.
    #[test]
    fn demo_document_band_starts_right_of_the_back_control() {
        let app = WebViewDemoApp::new().expect("demo app init");
        assert_eq!(app.webview.x as usize, back_button::X + back_button::SIZE);
        assert_eq!(
            (app.webview.x + app.webview.w) as usize,
            viewport::LOGICAL_WIDTH,
            "an inset band that keeps its width overhangs the panel's right edge"
        );
    }

    /// Back unwinds a fed document and only then leaves.
    #[test]
    fn back_control_unwinds_a_deeper_document() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        assert!(app.at_first_screen());

        app.feed_fetched_content(
            "https://interos.org/news",
            200,
            "text/html",
            FETCHED_DEMO_BYTES,
        )
        .expect("feed content");
        assert!(!app.at_first_screen());

        app.show_first_screen().expect("unwind to the demo page");
        assert!(app.at_first_screen());
        assert_eq!(app.webview.title(), "InterWebView Demo");
        assert_eq!(app.webview.current_url(), DEMO_URL);
        assert_eq!(app.webview.scroll().offset, 0);
    }
}
