// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Demonstrator library for InterWebView embedding platform.

#![no_std]

pub use interos_std as std;
use interos_std::design::{back_button, home_band, monochrome, viewport};
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

/// The demo's first document: the page the pane opens with.
///
/// WHY it is this long: the controller lays out one line box per heading and per
/// paragraph, and the pane is 1256 rows of them. The page this app shipped with
/// held one heading and two lines -- 158 rows, measured -- so a reader who opened
/// the demo saw a title, two sentences and eleven hundred rows of background. A
/// dead panel is not a demonstration. Every line below is a statement the app can
/// prove from its own code, and together they lay out 1339 rows: the pane is full
/// to its last row and the page runs 83 rows past its floor at the size this app
/// draws at, which is what makes the drag worth having.
///
/// WHY the shape is three headings over twenty-nine paragraphs: thirty-two boxes
/// is all the controller takes, so thirty-two is the tallest page this pane can
/// hold; and it pulls every `<h1>` out of the string before it looks at a
/// paragraph, so a body line written *between* two headings is handed over and
/// never laid out. The headings therefore stand together at the top, as a
/// masthead, and the prose follows them.
///
/// WHY the sentences are this short: the extractor sets one line per box and
/// wraps nothing, so a line wider than the column the pane has is cut off at its
/// right edge, and a box longer than sixty-four bytes is cut off by the
/// controller itself. `no_document_line_is_cut_off_at_the_pane_edge` holds both
/// limits, and `demo_document_holds_every_box_its_source_carries` holds the shape.
///
/// WHY the copy is about this component: the pane is the platform's truth
/// surface for the embedded web view. It says what renders here, what does not,
/// what no transport is attached to, and what the reader can press.
pub const DEMO_HTML: &str = r#"
<!DOCTYPE html>
<html>
<head><title>InterWebView Demo</title></head>
<body>
<h1>InterWebView Demo</h1>
<h1>Embedded view, Ring 3</h1>
<h1>These bytes, no wire</h1>
<p>Every line here is a box the</p>
<p>controller read out of HTML shipped</p>
<p>inside this app's own binary. It took</p>
<p>them with load_html, once, before</p>
<p>the pane's first frame was drawn.</p>
<p>Nothing on this page came off a</p>
<p>network: this demo's policy refuses</p>
<p>http loads, and the refusal it hands</p>
<p>back is named, not hidden.</p>
<p>The controller is a native layout</p>
<p>extractor, not a browser engine. It</p>
<p>reads heading and paragraph boxes,</p>
<p>one line each, thirty-two at most,</p>
<p>and it puts the headings first.</p>
<p>It draws no CSS, no flexbox, no grid,</p>
<p>no images and no subframes. It keeps</p>
<p>no cookies, no storage and no DOM.</p>
<p>Scripts, had this page carried any,</p>
<p>run bounded to two thousand steps,</p>
<p>and no native bridge is granted here.</p>
<p>Tap the page: a second fixture comes</p>
<p>in through the feeder call,</p>
<p>load_url_content, with a status and</p>
<p>a type its caller passed. Tap again</p>
<p>to have this page back.</p>
<p>Drag the page: it moves where the</p>
<p>document runs past the pane's floor.</p>
<p>The chip unwinds the other page, and</p>
<p>the pill at the foot ends the demo.</p>
</body>
</html>
"#;

/// The demo's second document, handed over by the feeder call rather than by
/// `load_html`.
///
/// WHY a second page at all: `load_url_content` is the other half of what this
/// component does -- a caller passes bytes, a status and a content type, and the
/// pane renders them -- and a demo that never calls it demonstrates half a
/// platform. The bytes are the demo's own, and the page says so in its own
/// words: the copy that stood here before claimed "real network content", and
/// nothing on this device has ever fetched it.
pub const FED_HTML: &str = r#"
<!DOCTYPE html>
<html>
<head><title>Fed Fixture</title></head>
<body>
<h1>Fed Fixture</h1>
<h1>Second page here</h1>
<h1>Bytes, not a reply</h1>
<p>This page arrived through the demo's</p>
<p>own feeder call, load_url_content,</p>
<p>with a status and a content type the</p>
<p>caller passed, from bytes compiled</p>
<p>into this binary. No response came</p>
<p>in over a network to be handed over.</p>
<p>That call exists for a host that has</p>
<p>already fetched a document: it passes</p>
<p>the bytes, the status and the type</p>
<p>in here, and the pane lays them out</p>
<p>the way it laid out the first page,</p>
<p>under the same limits.</p>
<p>No fetch host is attached to this</p>
<p>device: the demo feeds its own bytes.</p>
<p>The address the pane is at is the</p>
<p>demo's own name for these bytes. It</p>
<p>is not a site and nothing was looked</p>
<p>up, and no request was made to fail.</p>
<p>What the load really recorded, in the</p>
<p>controller's own event ring: a title</p>
<p>change, then a page finished carrying</p>
<p>the status the caller handed in.</p>
<p>There is no cache behind this call,</p>
<p>no history stack and no retry.</p>
<p>Tap the page to have the first</p>
<p>fixture back; the chip above does</p>
<p>the same thing, and puts the page at</p>
<p>its first line again.</p>
<p>Both fixtures are the demo's own.</p>
</body>
</html>
"#;

/// The address the demo's second fixture is shown at.
///
/// WHY an `interos://` address and not an `https://` one: the bytes are shipped
/// inside this binary and handed over by the feeder call. An `https` origin on
/// the screen would name a site the device never contacted, and the pane would
/// be reporting a navigation it did not make.
pub const FED_URL: &str = "interos://demo/fed-fixture.html";

/// One pixel of the demo's surface: the platform's back control where it stands,
/// then the home pill the band answers, then the document's own pixel where the
/// controller claims the position, and the page background everywhere else.
///
/// WHY this lives in the library and not in the binary: the pixel rule *is* the
/// screen the reader gets, and a rule that can only be exercised by booting the
/// device is a rule nobody can write a failing test against. The binary walks
/// the rows and hands each position to this function.
///
/// WHY the app has to answer the rows the controller does not: the kernel only
/// publishes a surface whose pixels have all been written, and
/// `InterWebView::render` writes the band it was configured with -- rows
/// `y .. y + h` of a 1440-row canvas. The rows outside it stay untouched, so the
/// frame is never marked complete, `composite` skips the surface, and the demo
/// shows nothing at all while logging that it presented a frame.
///
/// WHY the control is asked before the document: the webview band starts at row
/// 84 and the control owns rows 56..104, so the top of the chip overlaps the
/// document. A control that a page can paint over is a control that isn't there.
///
/// WHY the pill is the SDK's ink and not a shape here: the demo painted nothing
/// at the foot of the panel, so the only way off the document was a gesture the
/// screen never advertised.
pub fn demo_pixel(x: usize, y: usize, page: &InterWebView) -> u32 {
    if let Some(c) = back_button::pixel_for_theme(x, y, &THEME) {
        return c;
    }
    if let Some(c) = home_band::pixel(x, y, THEME.on_surface_variant) {
        return c;
    }

    let px0 = page.x as usize;
    let py0 = page.y as usize;
    let px1 = px0 + page.w as usize;
    let py1 = py0 + page.h as usize;

    if x >= px0 && x < px1 && y >= py0 && y < py1 {
        let c = page.get_pixel(x, y);
        // Zero alpha is the controller's own way of saying the position is not
        // its ink; it is never written into an opaque surface.
        if c & 0xFF000000 != 0 {
            return c;
        }
    }
    PAGE_BG
}

/// The event kinds the kernel's own `InputEventKind` numbers put on the wire,
/// as `InputEventPoll` copies them into `InputEvent::kind`.
///
/// WHY spelled out here: the numbers are the kernel's ABI, and an application
/// that reads a `4` as "the back gesture" instead of "the finger moved" throws
/// the reader out of the app on every scroll. The names are the one place this
/// screen can say which meaning it is drawing on.
pub const KERNEL_TAP: u8 = 1;
pub const KERNEL_PAN: u8 = 4;
pub const KERNEL_FLING: u8 = 10;

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

    /// Puts the demo's second fixture on the pane through the feeder call.
    ///
    /// WHY the demo calls this itself: the feeder is the component's other load
    /// path, and the only honest way a demonstrator can show it on a device with
    /// no fetch host attached is to feed the bytes it holds and say that is what
    /// happened -- which is what the page says.
    pub fn load_fed_fixture(&mut self) -> Result<(), &'static str> {
        self.feed_fetched_content(FED_URL, 200, "text/html", FED_HTML.as_bytes())
    }

    /// Feeds caller-supplied bytes and a status into the embedded webview.
    ///
    /// WHY the wording: this is the seam a host that has already fetched a
    /// response would use. The component performs no fetch of its own, so the
    /// status a caller passes is the status the caller observed, and nothing
    /// here invents one.
    pub fn feed_fetched_content(
        &mut self,
        url: &str,
        status: u16,
        content_type: &str,
        body: &[u8],
    ) -> Result<(), &'static str> {
        self.webview.load_url_content(url, status, content_type, body)
    }

    /// Whether this position is on the document, rather than on the chrome that
    /// stands above, below or beside it.
    pub fn over_document(&self, x: usize, y: usize) -> bool {
        let page = &self.webview;
        x >= page.x as usize
            && x < (page.x + page.w) as usize
            && y >= page.y as usize
            && y < (page.y + page.h) as usize
    }

    /// Answers one input event with the document, and says whether the pane has
    /// to be painted again.
    ///
    /// WHY the app reads the kernel's kinds rather than handing the event to the
    /// controller's `handle_touch`: that call moves the page on a `2`, and the
    /// kernel numbers a `2` as a double tap, whose event carries no delta at
    /// all. A drag reaches an application as a `4`, so a demo that delegated the
    /// gesture had a page it could never move -- and the page this app opens
    /// with is taller than its pane, so the bottom of it was unreachable. The
    /// controller's own path is still asked for every kind the app does not
    /// claim, so a component that learns to answer a gesture keeps doing it.
    ///
    /// WHY a tap turns over the document: the demo has two load paths to show
    /// and no chrome of its own to press, and a pane that renders a page is
    /// where a reader expects a press to mean something.
    ///
    /// WHY a failed load answers `false`: the pane keeps the page it already
    /// has, and an app that repainted after a refusal would be showing a
    /// document it never loaded.
    pub fn handle_document_input(&mut self, input: &interos_std::InputEvent) -> bool {
        if !self.over_document(input.x as usize, input.y as usize) {
            return false;
        }
        match input.kind {
            KERNEL_PAN | KERNEL_FLING => {
                self.webview.scroll_mut().apply_drag(input.dy);
                true
            }
            KERNEL_TAP => {
                let res = if self.at_first_screen() {
                    self.load_fed_fixture()
                } else {
                    self.show_first_screen()
                };
                res.is_ok()
            }
            _ => self.webview.handle_touch(input),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interos_std::webview::{self, WebViewEvent, WebViewState};

    /// One kernel-shaped event at one position of the panel, so the tests hand
    /// the app exactly what `sys_input_event_poll` copies out of the kernel's
    /// gesture record.
    fn press(kind: u8, x: u32, y: u32, dy: i32) -> interos_std::InputEvent {
        interos_std::InputEvent {
            kind,
            x,
            y,
            dx: 0,
            dy,
            scale_milli: 1000,
        }
    }

    /// The row a document's first line box sits on, measured out of the
    /// controller rather than copied from it.
    ///
    /// WHY measured: `TEXT_INSET_Y` is the module's private number, and the
    /// height of a laid-out page is that inset plus one box per line. A test that
    /// restated it would pass on the day the controller moved its text and fail
    /// on the day it did not, which is the opposite of what a pin is for.
    fn text_inset_y() -> usize {
        let mut page = InterWebView::create_webview(WebViewConfig::default_portrait())
            .expect("probe webview");
        page.load_html("<h1>x</h1>", None).expect("probe load");
        page.scroll().content_h - webview::line_box(true)
    }

    /// The document boxes a fixture source carries: how many headings, how many
    /// paragraphs, and the longest run of text between them, in bytes.
    ///
    /// WHY read out of the source and not out of the controller: the point is to
    /// catch a box the controller was handed and did not keep. A test that asked
    /// the controller how many lines it holds can only ever agree with itself.
    fn fixture_boxes(html: &str) -> (usize, usize, usize) {
        let mut heads = 0usize;
        let mut paras = 0usize;
        let mut longest = 0usize;
        let mut cur = html;
        loop {
            let h = cur.find("<h1>");
            let p = cur.find("<p>");
            let (start, open, close) = match (h, p) {
                (Some(hs), Some(ps)) if hs <= ps => (hs, 4usize, "</h1>"),
                (Some(hs), None) => (hs, 4, "</h1>"),
                (None, Some(ps)) => (ps, 3, "</p>"),
                // A paragraph box written ahead of a heading box. The extractor
                // pulls every heading out of the string before it looks at a
                // paragraph, so this shape silently loses the paragraph. Stop
                // counting here and let the totals below say so.
                (Some(_), Some(_)) => break,
                (None, None) => break,
            };
            let text_start = start + open;
            let rel = match cur[text_start..].find(close) {
                Some(r) => r,
                None => break,
            };
            let text = &cur[text_start..text_start + rel];
            if close == "</h1>" {
                heads += 1;
            } else {
                paras += 1;
            }
            if text.len() > longest {
                longest = text.len();
            }
            cur = &cur[text_start + rel + close.len()..];
        }
        (heads, paras, longest)
    }

    /// How many rows the document inks in the pane's last column.
    ///
    /// WHY the last column answers clipping: the controller reports a pixel only
    /// inside its own band, so a run that outgrew the band is not drawn narrower
    /// -- it is cut. The cut always lands on this column.
    fn edge_ink_rows(page: &InterWebView) -> usize {
        let last_x = (page.x + page.w - 1) as usize;
        let mut rows = 0usize;
        for y in page.y as usize..(page.y + page.h) as usize {
            if demo_pixel(last_x, y, page) != PAGE_BG {
                rows += 1;
            }
        }
        rows
    }

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

    /// The feeder path, recorded as what it is: the demo's own bytes, handed in
    /// with the status the demo passed, shown at an address the demo named.
    ///
    /// WHY the assertion moved off `https://interos.org/news` and "Fetched
    /// Article": the copy that stood here claimed real network content, and this
    /// device has no fetch host attached to hand a response over. A test that
    /// pinned that claim pinned a story, not a behaviour -- and the behaviour
    /// (bytes in, document out, the caller's status carried through) is what is
    /// asserted now, over an address this app owns.
    #[test]
    fn demo_app_feeds_fetched_content_truthfully() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        // Clear initial events
        while app.webview.poll_event().is_some() {}

        let res = app.feed_fetched_content(FED_URL, 200, "text/html", FED_HTML.as_bytes());
        assert!(res.is_ok());
        assert_eq!(app.webview.title(), "Fed Fixture");
        assert_eq!(app.webview.current_url(), FED_URL);
        assert_eq!(app.webview.state(), WebViewState::Loaded);

        let mut got_title = false;
        let mut got_finished = false;
        while let Some(evt) = app.webview.poll_event() {
            match evt {
                WebViewEvent::TitleChanged(..) => got_title = true,
                WebViewEvent::PageFinished(url_buf, len, status) => {
                    assert_eq!(status, 200);
                    let url_str = core::str::from_utf8(&url_buf[..len]).unwrap();
                    assert_eq!(url_str, FED_URL);
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

        app.load_fed_fixture().expect("hand over the second fixture");
        assert!(!app.at_first_screen());

        app.show_first_screen().expect("unwind to the demo page");
        assert!(app.at_first_screen());
        assert_eq!(app.webview.title(), "InterWebView Demo");
        assert_eq!(app.webview.current_url(), DEMO_URL);
        assert_eq!(app.webview.scroll().offset, 0);
    }

    /// The frame the demo opens with is a document, not a black panel.
    ///
    /// WHY it counts pixels and not lines: the complaint against this screen was
    /// that a reader who opened it found a title, two lines and then nothing for
    /// the rest of the surface. That is a statement about the pixels, so the
    /// test asks about the pixels -- every position of the pane below the first
    /// lines, through the same function the binary paints with.
    ///
    /// WHY the pane and not the whole canvas: the home pill is ink, the back chip
    /// is ink, and neither says a page was rendered. Only the rows the controller
    /// owns can answer that.
    ///
    /// WHY the number in the message is worth reading: the page this app shipped
    /// with put 0 inked pixels below its first lines and 100,134 with the fixture
    /// this file carries now. The threshold sits an order of magnitude under the
    /// second number so a line that is reworded shorter still passes, and a pane
    /// that has gone back to being a background still does not.
    #[test]
    fn demo_opening_frame_paints_a_document_down_the_pane() {
        let app = WebViewDemoApp::new().expect("demo app init");
        let pane_top = app.webview.y as usize + 220;
        let pane_end = (app.webview.y + app.webview.h) as usize;

        let mut ink = 0usize;
        for y in pane_top..pane_end {
            for x in 0..viewport::LOGICAL_WIDTH {
                if demo_pixel(x, y, &app.webview) != PAGE_BG {
                    ink += 1;
                }
            }
        }

        assert!(
            ink > 20_000,
            "the opening frame put {} inked pixels in the pane below its first \
             lines: the reader opens a black panel, not a page",
            ink
        );
    }

    /// Every pixel of the canvas the demo creates is answered with an opaque
    /// colour.
    ///
    /// WHY the whole canvas and not the pane: the controller paints its band and
    /// nothing else, and the kernel only presents a surface whose pixels have all
    /// been written. An app that renders `webview.render` and stops leaves
    /// nineteen hundred rows of the panel untouched, the frame is never marked
    /// complete, `composite` skips the surface, and the reader sees nothing at all
    /// while the log says a frame was presented. `demo_pixel` is what closes that
    /// gap, so this is the rule stated where it can fail.
    #[test]
    fn the_demo_answers_every_pixel_of_the_canvas_it_owns() {
        let app = WebViewDemoApp::new().expect("demo app init");
        let mut untouched = 0usize;
        for y in 0..viewport::LOGICAL_HEIGHT {
            for x in 0..viewport::LOGICAL_WIDTH {
                if demo_pixel(x, y, &app.webview) & 0xFF000000 == 0 {
                    untouched += 1;
                }
            }
        }
        assert_eq!(
            untouched, 0,
            "{} of the canvas's {} positions are left transparent, and a frame \
             with an untouched pixel is never presented",
            untouched,
            viewport::LOGICAL_WIDTH * viewport::LOGICAL_HEIGHT
        );
    }

    /// The page the demo opens with is longer than the window it opens in.
    ///
    /// WHY the controller's own layout numbers: `content_h` is what the drag in
    /// `ScrollState` is capped against, so a document shorter than its band is a
    /// document with nothing to reach -- and a pane whose last nine tenths is
    /// background is the black screen this test exists to keep away.
    #[test]
    fn demo_document_is_taller_than_the_pane_it_opens_in() {
        let app = WebViewDemoApp::new().expect("demo app init");
        let scroll = app.webview.scroll();
        assert!(
            scroll.content_h > scroll.viewport_h,
            "the demo document lays out {} rows inside a {} row pane, so the \
             pane is mostly empty and there is nothing to scroll to",
            scroll.content_h,
            scroll.viewport_h
        );
        assert!(scroll.max_offset() > 0);
    }

    /// The page the reader opens reaches the bottom of its pane.
    ///
    /// WHY the deepest inked row and not a line count: the defect this screen
    /// carried was a shape on the panel -- a page that stopped a tenth of the way
    /// down -- and the only thing that can be said about it without reading the
    /// controller's private geometry is where the last painted row of the document
    /// lies. The margin below that row is the background the reader sees.
    ///
    /// WHY not the pane's very last row: a line box reserves the typeface's
    /// descent under its ink, so the final row of a full page is the box, not the
    /// glyph. Three line boxes is more slack than any box the ladder draws.
    #[test]
    fn demo_page_runs_to_the_floor_of_its_pane() {
        let app = WebViewDemoApp::new().expect("demo app init");
        let pane_end = (app.webview.y + app.webview.h) as usize;

        let mut deepest = 0usize;
        for y in app.webview.y as usize..pane_end {
            for x in app.webview.x as usize..(app.webview.x + app.webview.w) as usize {
                if demo_pixel(x, y, &app.webview) != PAGE_BG {
                    deepest = y + 1;
                    break;
                }
            }
        }

        let floor = pane_end - 3 * webview::line_box(false);
        assert!(
            deepest >= floor,
            "the demo's document paints its last ink at row {} of a pane that \
             ends at row {}; {} rows of background sit below the page",
            deepest,
            pane_end,
            pane_end - deepest
        );
    }

    /// Every box a fixture carries reaches the page the reader gets.
    ///
    /// WHY this is worth a test of its own: the extractor walks a document by
    /// pulling each `<h1>` out of the whole remaining string before it looks at a
    /// paragraph, so a body line written between two headings is handed over and
    /// never laid out, and nothing about the pane says so. The height of the page
    /// is the one number that moves when a box is dropped, and it is the number
    /// the drag is capped against.
    ///
    /// WHY the shape is pinned as well as the count: thirty-two boxes is the
    /// controller's own ceiling, and a fixture longer than that is a fixture whose
    /// last lines were never going to appear.
    #[test]
    fn demo_document_holds_every_box_its_source_carries() {
        let app = WebViewDemoApp::new().expect("demo app init");

        for html in [DEMO_HTML, FED_HTML] {
            let (heads, paras, longest) = fixture_boxes(html);
            assert!(
                heads + paras <= webview::MAX_TEXT_LINES,
                "a fixture carries {} boxes and the controller lays out {} at most",
                heads + paras,
                webview::MAX_TEXT_LINES
            );
            assert!(
                longest <= webview::LINE_STR_MAX,
                "a document box holds {} bytes and the controller keeps {} of it",
                longest,
                webview::LINE_STR_MAX
            );
            assert_eq!(
                html.matches("<p").count(),
                html.matches("<p>").count(),
                "a paragraph box was written with attributes, which the \
                 fixture counter above does not read and the extractor does"
            );
        }

        let (heads, paras, _) = fixture_boxes(DEMO_HTML);
        let expected = text_inset_y()
            + heads * webview::line_box(true)
            + paras * webview::line_box(false);
        assert_eq!(
            app.webview.scroll().content_h,
            expected,
            "the demo's first page is on the pane, and its {} heading and {} \
             paragraph boxes lay out to {} rows, but the controller reports {} \
             -- a box the extractor skipped never reaches the page",
            heads,
            paras,
            expected,
            app.webview.scroll().content_h
        );
    }

    /// The second fixture is held to the same rule as the first.
    ///
    /// WHY a second test: the fed page reaches the reader only through a press,
    /// so a box dropped from it is a box nobody sees on the panel -- and the page
    /// under it is the one the demo opened with.
    #[test]
    fn fed_document_holds_every_box_its_source_carries() {
        let inset = text_inset_y();
        let mut page = InterWebView::create_webview(WebViewConfig::default_portrait())
            .expect("probe webview");
        page.load_url_content(FED_URL, 200, "text/html", FED_HTML.as_bytes())
            .expect("probe feed");

        let (heads, paras, longest) = fixture_boxes(FED_HTML);
        assert!(heads + paras <= webview::MAX_TEXT_LINES);
        assert!(longest <= webview::LINE_STR_MAX);
        let expected = inset + heads * webview::line_box(true) + paras * webview::line_box(false);
        assert_eq!(
            page.scroll().content_h,
            expected,
            "the fed page's {} heading and {} paragraph boxes lay out to {} rows \
             but the controller reports {}",
            heads,
            paras,
            expected,
            page.scroll().content_h
        );
    }

    /// No line of either fixture is cut off where the pane ends.
    ///
    /// WHY the pixels of the last column: the controller answers a position only
    /// inside its own band, so a run wider than the band is not laid out smaller
    /// -- it is sliced, and the reader is left holding half a word at the panel's
    /// edge. Nothing in the API reports that; the inked edge does.
    #[test]
    fn no_document_line_is_cut_off_at_the_pane_edge() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        assert_eq!(
            edge_ink_rows(&app.webview),
            0,
            "the demo's first page inks the pane's last column, so a line of it \
             is cut off at the panel's edge"
        );

        app.load_fed_fixture().expect("hand over the second fixture");
        assert_eq!(
            edge_ink_rows(&app.webview),
            0,
            "the fed page inks the pane's last column, so a line of it is cut off \
             at the panel's edge"
        );
    }

    /// A press on the page turns the demo's two load paths over.
    ///
    /// WHY the app answers the kernel's `1` rather than the controller's touch
    /// path: the pane has no chrome of its own here, and a document that ignores
    /// every press gives the reader nothing to do with it.
    #[test]
    fn a_press_on_the_page_turns_over_its_document() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        let (x, y) = (400u32, 500u32);
        assert!(app.over_document(x as usize, y as usize));

        assert!(
            app.handle_document_input(&press(KERNEL_TAP, x, y, 0)),
            "a tap on the page did not ask for a repaint"
        );
        assert_eq!(app.webview.current_url(), FED_URL);
        assert!(!app.at_first_screen());

        assert!(app.handle_document_input(&press(KERNEL_TAP, x, y, 0)));
        assert_eq!(app.webview.current_url(), DEMO_URL);
        assert!(app.at_first_screen());
    }

    /// A drag inside the page moves the page.
    ///
    /// WHY this is a test and not an assumption: the kernel numbers a drag as a
    /// `4` and the controller's own touch path moves the document on a `2`, which
    /// the kernel calls a double tap and which carries no delta at all. An app
    /// that delegated the gesture to the controller had a page it could not move.
    ///
    /// WHY the drag is capped by the page rather than by the gesture: the offset
    /// a reader can reach is the document's height minus the pane's, and the demo
    /// asserts the bottom line is on the page once the drag has run out.
    #[test]
    fn a_drag_inside_the_page_moves_it() {
        let mut app = WebViewDemoApp::new().expect("demo app init");
        let travel = app.webview.scroll().max_offset();
        assert!(
            travel > 0,
            "the page the demo opens with has nothing to scroll to"
        );

        // Half the page's travel, so the drag lands inside the document rather
        // than against its end and the assertion is about the gesture.
        let half = travel / 2;
        assert!(app.handle_document_input(&press(KERNEL_PAN, 400, 500, -(half as i32))));
        assert_eq!(app.webview.scroll().offset, half);

        assert!(app.handle_document_input(&press(KERNEL_PAN, 400, 500, -4000)));
        assert_eq!(
            app.webview.scroll().offset,
            travel,
            "a drag past the end of the page should stop at its last line"
        );

        assert!(app.handle_document_input(&press(KERNEL_FLING, 400, 500, 4000)));
        assert_eq!(app.webview.scroll().offset, 0);
    }

    /// A press that is not on the page is not answered by the page.
    ///
    /// WHY the header and the foot: the chip and the pill own those rows, and a
    /// document that swallowed a press over them would take the control's answer
    /// away from the control.
    #[test]
    fn a_press_off_the_page_moves_nothing() {
        let mut app = WebViewDemoApp::new().expect("demo app init");

        assert!(
            !app.handle_document_input(&press(KERNEL_PAN, 400, 40, -120)),
            "a drag over the header was answered by the document"
        );
        assert_eq!(app.webview.scroll().offset, 0);

        assert!(
            !app.handle_document_input(&press(KERNEL_TAP, 360, 30, 0)),
            "a tap over the header turned the document"
        );
        assert!(app.at_first_screen());

        let below = (app.webview.y + app.webview.h) as u32 + 40;
        assert!(
            !app.handle_document_input(&press(KERNEL_TAP, 360, below, 0)),
            "a tap at the foot of the panel turned the document"
        );
        assert!(app.at_first_screen());
    }

    /// The page names no site, and claims no content this device cannot have.
    ///
    /// WHY the strings are read rather than the behaviour tested: what stood here
    /// before said "real network content delivered via the load_url_content
    /// feeder", and the feeder had never been handed a response -- there is no
    /// fetch host on the device. A behaviour test cannot catch a sentence, so this
    /// one reads the sentences the reader is shown.
    #[test]
    fn the_fixtures_claim_no_content_the_device_cannot_have_reached() {
        for html in [DEMO_HTML, FED_HTML] {
            assert!(
                !html.contains("https://") && !html.contains("http://"),
                "a document page names a site, and nothing on this device can \
                 have reached one"
            );
        }
        for url in [DEMO_URL, FED_URL] {
            assert!(
                !url.starts_with("http"),
                "the demo shows the address {}, which reads as a page the pane \
                 navigated to over a network",
                url
            );
        }
        assert!(!FED_HTML.contains("Fetched Article"));
        assert!(!DEMO_HTML.contains("Fetched Article"));
    }

    /// The package descriptor does not carry the browser's icon token.
    ///
    /// WHY this is only half of defect B, and said plainly: the app drawer does
    /// not read this file. It asks the launcher's icon table about the package id
    /// the kernel's own table hands it, and that table currently answers
    /// `IconKind::Browser` for `org.interos.webview_demo` -- a globe. The token
    /// below is what the descriptor ought to name, and the launcher and kernel
    /// lines that have to follow it are reported with it; neither is this crate's
    /// to change.
    #[test]
    fn the_package_descriptor_is_not_the_browser_s() {
        let manifest = include_str!("../manifest.json");
        for token in ["\"WEB\"", "\"BROW\"", "\"WEBV\"", "\"Browser\""] {
            assert!(
                !manifest.contains(token),
                "the descriptor names the icon {}, which the launcher's table \
                 already draws as the browser's globe",
                token
            );
        }
        assert!(
            manifest.contains("\"icon\": \"EMBED\""),
            "the descriptor does not state the icon this app asks to be drawn as"
        );
    }
}
