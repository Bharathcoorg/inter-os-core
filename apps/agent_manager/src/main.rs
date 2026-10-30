//! The InterOS Agent Manager: a native portrait phone app.
//!
//! WHAT THIS IS: a Ring-3 supervisor app whose whole job is to report what
//! the device actually holds. It is not a dashboard, it is not a console, and
//! it does not simulate a platform. Every number on every screen came out of
//! a query the kernel answered, and every screen that could not get an answer
//! says which of the three things happened instead.
//!
//! THE SHAPE: one column, a status bar, a segmented control of five surfaces,
//! a scrollable body, and a bottom action bar. That is the shape the platform's
//! other apps use on a 720x1440 logical screen, and the metrics come from
//! the platform's own design system -- `interos_std::design::theme` for the
//! roles, the type scale, the spacing, the radii and the touch minimum, and
//! `interos_std::design::components` for the card and row treatments.
//!
//! THE THEME: `Theme::NEUTRAL` -- black, white and neutral grey. The chrome
//! carries no hue at all. Colour appears in exactly three places: the app's
//! own mark, the identity of the selected model, and a status the user has to
//! be able to see (allowed, denied, unavailable).
//!
//! WHAT THIS APP DOES NOT CLAIM:
//! - It does not claim a system-wide integration. The launcher-to-service
//!   path, the authorisation and the retrieval are not wired up, and the
//!   Runtime surface says so in the platform's own terms.
//! - It does not claim a model is installed because a vendor sells one. The
//!   catalogue's install state is a measurement.
//! - It does not print a passing self-test it did not run. Every row of the
//!   self-test holds the value the platform's own function returned.

#![no_std]
#![no_main]

use core::panic::PanicInfo;

use interos_std as std;
use std::design::components;
use std::design::font;
use std::design::monochrome::{self, Icon};
use std::design::primitives::Rect;
use std::design::scroll::ScrollState;
use agent_manager::theme::THEME;

use agent_manager::{
    format_threshold, format_u32, granted_capability_count, load_saved_provider,
    probe_capability_grants, providers, resolve_inference_service, save_preferred_provider,
    service_name, AgentTool, AgentToolOutcome, CapabilityGrant, ChatRole, ChatSession,
    ConsentOutcome, DeviceState, InstallState, ModelProbeStatus, PackageInstallOutcome,
    PackageRemoveOutcome, ProbeStatus, Provider, RegistryPackageState, RuntimeBinding,
    RuntimeDiagnostics, SearchConfiguration, SelfTest, ServiceState, StreamChunk,
    Surface, ToolAuthority, transact_package_install, transact_package_remove,
    CAPABILITY_COUNT, CHAT_PROMPT_MAX, HOST_BRIDGE, LOCAL_GENERATIVE,
    MATRYOSHKA_DEFAULT_DIM, MAX_RESULTS_DEFAULT, SIMILARITY_DEFAULT_Q15, TOOL_LABEL_MAX,
};

// -----------------------------------------------------------------------------
// The screen
// -----------------------------------------------------------------------------

const W: usize = 720;
const H: usize = 1440;
const CHUNK: usize = 4096;

/// The theme this app draws with.
///
/// WHY a constant and not a lookup: the theme is a property of the app's
/// identity, not of a setting the user may change underneath it. The neutral
/// variant is the one the design system offers for an interface whose colour
/// is reserved.

/// The horizontal gutter the platform's other apps use.
const GUTTER: usize = 24;
/// The width of the content column.
const CONTENT_W: usize = W - 2 * GUTTER;

/// The height of the platform's band, taken from the platform.
const STATUS_BAR_HEIGHT: usize = std::design::viewport::STATUS_BAR_HEIGHT;
/// The first row this app's own chrome may paint in.
const HEADER_TOP: usize = std::design::viewport::HEADER_TOP;
/// The app bar's height.
///
/// WHY this band is the app's bar and not the platform's status bar: the
/// platform's status bar owns the top inset and this app's bar is what lives
/// under it -- the way out, the title and the two controls the showcases put
/// there.
///
/// WHY it reaches to 112: the way out is the platform's own control, and the
/// control is fixed at the row below the band and forty-eight tall. A bar of
/// the height this file used to carry cut the control in half, because the
/// segmented control beneath it answered the pixels the bar had given up.
const APPBAR_H: usize = 112;
/// The size of an app bar icon button.
const ICON_BTN: usize = 44;
/// The gap between two app bar icon buttons.
const ICON_BTN_GAP: usize = 8;
/// The app bar's title's left edge.
const APPBAR_TITLE_X: usize = 88;
/// The row the app bar's controls are centred on.
const APPBAR_ROW: usize = HEADER_TOP + std::design::back_button::SIZE / 2;
/// The top of an app bar icon button.
///
/// WHY it is not the bar's own middle: the bar now carries the band's rows as
/// well, so a control centred on the whole bar sits above the platform's back
/// button instead of beside it.
const ICON_BTN_TOP: usize = APPBAR_ROW - ICON_BTN / 2;
/// The app bar's title's line box top.
///
/// WHY it is solved from the baseline rather than from the box: the box is
/// taller than the forty-eight the control stands in, so centring the box
/// would leave the bar's row and pushing it down to clear the band drops the
/// letters below the control. Centring the caps -- half a cap height above the
/// row and half below -- is what lines the word up with the button next to it.
const APPBAR_TITLE_Y: usize = APPBAR_ROW - CAP_TITLE_ASCENT + CAP_TITLE as usize / 2;
/// The floating action button's radius.
const FAB_R: usize = 28;
/// The floating action button's margin from the body's edge.
const FAB_MARGIN: usize = 16;
/// The padding inside a chat bubble.
const BUBBLE_PAD: usize = 12;
/// The chat surface's composer band.
const COMPOSER_H: usize = 76;
/// The composer's field height.
/// The composer's field height.
///
/// WHY it follows the type rather than being a number: the platform's
/// type scale names a line box, and a field that is shorter than the line
/// it holds cannot hold it. A fixed height here broke the moment the
/// ladder's rungs moved, because the arithmetic underflowed.
const COMPOSER_FIELD_H: usize = LINE_BODY + 28;
/// The composer's send button radius.
const COMPOSER_SEND_R: usize = 24;

/// The segmented control's band.
///
/// WHY it follows the bar rather than sitting at a number of its own: the bar
/// grew when it took the platform's back control, and a band pinned at the old
/// bottom was drawn over the control it no longer clears.
const TAB_Y: usize = 120;
const TAB_H: usize = 44;
const TAB_GAP: usize = 5;

/// The scrollable body's band.
const BODY_TOP: usize = 180;
/// WHY at compile time: the three bands above are written as numbers so the
/// platform's layout contract can read them, and a number is only honest while
/// it still matches the geometry the controls are actually placed from.
const _: () = assert!(
    APPBAR_H == HEADER_TOP + std::design::back_button::SIZE + 8
        && TAB_Y == APPBAR_H + 8
        && BODY_TOP == TAB_Y + TAB_H + 16
);
const BODY_BOTTOM: usize = 1284;
const BODY_H: usize = BODY_BOTTOM - BODY_TOP;
/// The bottom action bar's band.
const ACTION_TOP: usize = 1284;
const BUTTON_Y: usize = 1300;
const BUTTON_H: usize = 52;
const BUTTON_GAP: usize = 8;
const RESULT_Y: usize = 1364;

/// The home indicator.
const GESTURE_Y: usize = 1420;
const GESTURE_W: usize = 160;

// WHY the action bar starts where the body ends: the two bands share an
// edge, and a gap between them would be a strip of page that belongs to
// neither. The compiler checks the two numbers agree.
const _: () = assert!(ACTION_TOP == BODY_BOTTOM);

// The body model.
const CARD_PAD_X: usize = 20;
const CARD_PAD_TOP: usize = 22;
const CARD_PAD_BOTTOM: usize = 18;
const CARD_GAP: usize = 14;
/// The height of a card's heading row.
///
/// WHY the heading is given a row of its own: the heading is the name of
/// the thing the rows under it belong to, and a name that shares a line
/// with a measurement is a measurement.
const TITLE_ROW_H: usize = 40;
const TITLE_GAP: usize = 10;
/// The height of a row.
///
/// WHY a row is a touch target's height and not a line's height: a row a
/// person can read but cannot press is a label, and the platform's
/// minimum touch target is forty-four pixels.
/// A row's height. It follows the type for the same reason the composer's
/// field does.
const ROW_H: usize = LINE_BODY + 20;
const LINES_TOP: usize = CARD_PAD_TOP + TITLE_ROW_H + TITLE_GAP;

/// The label column's width in pixels.
/// The label column's width.
///
/// WHY 216 and not 300: the longest label the app uses is eighteen
/// characters, and a column wide enough for the longest label is a
/// column with room to spare. The old width spent a quarter of the
/// content column on white space, which is what made the rows read
/// as a table rather than as a phone's list.
const LABEL_W: usize = 216;
/// The gap between the label column and the value column.
///
/// WHY the gap is not zero: the face is proportional, and a label that
/// exactly fills its column touches the value beside it. A phone list
/// needs a gutter.
const LABEL_GAP: usize = 8;
/// The value column's left edge, in absolute x.
const VALUE_X: usize = GUTTER + CARD_PAD_X + LABEL_W + LABEL_GAP;
/// The value column's width in pixels.
const VALUE_W: usize = GUTTER + CONTENT_W - CARD_PAD_X - VALUE_X;
/// The width of a wide line's text.
const WIDE_W: usize = GUTTER + CONTENT_W - CARD_PAD_X - (GUTTER + CARD_PAD_X);

/// How many characters a wide line wraps at.
const WRAP_COLS: usize = 52;

/// The longest run the outline face will draw, in bytes.
///
/// WHY the app knows the face's limit: a run the face refuses is drawn by
/// the bitmap face at the bitmap's whole-cell size, and a paragraph drawn
/// in a different face from the label above it is a paragraph that looks
/// like it belongs to another screen.
const MAX_RUN_BYTES: usize = std::design::outline_text::KEY_MAX;
/// How many physical rows a wide line can have.
const WIDE_ROWS: usize = 4;

// The type scale rungs the app draws with.
//
// WHY two rungs and not seven: the guest's bitmap face rasterises a 16x24
// cell at integer scales only, so the sizes the face can actually draw are
// 24px and 48px. The platform's own apps build the hierarchy between the
// rungs that are not expressible out of spacing, weight and ink, and so
// does this one: the rows are the 24px rung, the headings are the 48px
// rung, and the difference between a heading and a row is the weight and
// the ink as much as it is the size.
/// The cap heights the design document names, in the units a reader sees.
///
/// WHY the app names them rather than a scale multiple: the platform's
/// type scale is a ladder of cap heights, and a scale multiple is a
/// ladder of multiples. One of them can be drawn exactly and the other
/// has to be guessed at.
const CAP_CAPTION: u32 = std::design::typography::CAPTION_CAP_PX;
const CAP_BODY: u32 = std::design::typography::BODY_CAP_PX;
const CAP_SECTION: u32 = std::design::typography::SECTION_CAP_PX;
const CAP_TITLE: u32 = std::design::typography::TITLE_CAP_PX;

/// The line heights that go with the cap heights.
const LINE_CAPTION: usize = std::design::typography::CAPTION_LINE_PX;
const LINE_BODY: usize = std::design::typography::BODY_LINE_PX;
/// The row the title rung's baseline sits on, below its line box's top.
///
/// WHY it comes from the ladder: the app's own `text` puts the baseline at
/// `y0 + ascent` with these exact face metrics, so a line placed from this
/// number has its ink where the place was, and not a few rows away.
const CAP_TITLE_ASCENT: usize = font::ascent_for_scale(std::design::typography::TITLE_SCALE);

/// The length of the band's clock lane, as the RTC formats it.
const CLOCK_LEN: usize = 5;

/// How many cards the body can hold.
///
/// WHY the budget is generous: a body that runs out of room drops the tail
/// of its content without saying so, and a screen that quietly hides half
/// of a provider list is a screen that lies about the device.
const MAX_CARDS: usize = 16;
/// How many lines the body can hold.
const MAX_LINES: usize = 64;
/// The longest label a line carries.
const LABEL_MAX: usize = TOOL_LABEL_MAX;
/// The longest value a line carries.
const VALUE_MAX: usize = 44;
/// The longest value a wide line carries.
const WIDE_MAX: usize = WRAP_COLS * 3;
/// The longest action result the app keeps.
const ACTION_LOG_MAX: usize = 120;
/// The longest card title.
const TITLE_MAX: usize = 30;

// -----------------------------------------------------------------------------
// The body model
// -----------------------------------------------------------------------------

/// How a piece of text is coloured.
///
/// WHY an enum and not a colour: the colour comes from the theme, and a
/// screen that carried its own colours would be a second design system. The
/// tone says what the text *means*; the theme says what that means looks
/// like.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Ordinary ink.
    Plain,
    /// Secondary ink.
    Muted,
    /// Something the platform allowed.
    Good,
    /// Something the platform refused.
    Bad,
    /// Something the platform could not answer.
    Unavailable,
    /// A brand or identity mark.
    Accent,
}

impl Tone {
    /// The colour the theme gives this tone.
    fn colour(self) -> u32 {
        match self {
            Self::Plain => THEME.on_surface,
            Self::Muted => THEME.on_surface_variant,
            Self::Good => THEME.success,
            Self::Bad => THEME.error,
            Self::Unavailable => THEME.warning,
            Self::Accent => THEME.accent,
        }
    }
}

/// The tone a transcript card's title carries.
///
/// WHY the roles are coloured differently: a conversation that is read as
/// a list of undifferentiated lines is hard to follow, and the two sides
/// The tone a transcript message's text carries.
///
/// WHY the platform's own notices are muted rather than coloured as
/// warnings: a notice that reports an absence is not a failure, and
/// painting it as one would make the screen cry wolf.
fn chat_role_ink(role: ChatRole) -> Tone {
    match role {
        ChatRole::User => Tone::Plain,
        ChatRole::Assistant => Tone::Plain,
        ChatRole::Notice => Tone::Muted,
    }
}

/// One line of the body.
#[derive(Clone, Copy)]
pub struct Line {
    /// The label in the left column.
    pub label: [u8; LABEL_MAX],
    /// The label's length.
    pub label_len: u8,
    /// The value in the right column, or the whole text of a wide line.
    pub value: [u8; WIDE_MAX],
    /// The value's length.
    pub value_len: u8,
    /// How the value is coloured.
    pub tone: Tone,
    /// Whether the value runs the full width under the label.
    pub wide: bool,
    /// Where each physical row of a wide line begins.
    ///
    /// WHY the row starts are stored and not computed: a wrap that breaks
    /// mid-word reads as a mistake, and finding the break points needs the
    /// whole string. They are found once, when the line is built.
    pub row_starts: [u8; WIDE_ROWS],
    /// How many physical rows the line has.
    pub row_count: u8,
    /// The action this line performs when tapped, or -1.
    pub action: i8,
    /// Whether the value is drawn as a status chip rather than as text.
    ///
    /// WHY a chip and not coloured text: a state that is written in colour
    /// is a state a person has to read, and a state that is written as a
    /// dot and a word is a state a person can find at a glance.
    pub chip: bool,
}

impl Line {
    const fn empty() -> Self {
        Self {
            label: [0; LABEL_MAX],
            label_len: 0,
            value: [0; WIDE_MAX],
            value_len: 0,
            tone: Tone::Plain,
            wide: false,
            row_starts: [0; WIDE_ROWS],
            row_count: 1,
            action: -1,
            chip: false,
        }
    }

    /// How many physical rows the line occupies.
    fn rows(&self) -> usize {
        if self.wide {
            let rows = self.row_count as usize;
            if rows == 0 {
                1
            } else {
                rows
            }
        } else {
            1
        }
    }

    /// The byte range of one physical row of a wide line.
    fn row_range(&self, row: usize) -> (usize, usize) {
        let start = self.row_starts[row] as usize;
        let end = if row + 1 < self.row_count as usize {
            self.row_starts[row + 1] as usize
        } else {
            self.value_len as usize
        };
        (start, end)
    }
}

/// Finds where a wide line's physical rows begin.
///
/// WHY the break is at a space: a row that ends in the middle of a word
/// reads as a typo, and a screen that breaks words looks like it does not
/// know how text works.
///
/// WHY the rows are capped: the line's value buffer holds a fixed number of
/// bytes, and a row that begins past the end of the buffer would be a row
/// of nothing.
fn wrap_rows(value: &[u8], cap_px: u32, max_px: usize) -> ([u8; WIDE_ROWS], u8) {
    let mut starts = [0u8; WIDE_ROWS];
    let mut count = 1usize;
    let mut row_start = 0usize;

    while row_start < value.len() && count < WIDE_ROWS {
        // WHY the row is measured and not counted: the platform's face is
        // proportional, so a row's width depends on the letters in it. A
        // character count wrapped every line at fifty-two characters, which
        // is less than half the column the caption scale can fill, so a
        // sentence that fitted on one row was broken into two and the space
        // beside it was left empty.
        let mut limit = row_start;
        let mut width = 0usize;
        while limit < value.len() {
            // WHY the row is capped in bytes as well as in width: a row
            // longer than the face's key is refused by the face and drawn
            // by the bitmap, and the bitmap draws at a whole cell.
            if limit - row_start >= MAX_RUN_BYTES {
                break;
            }
            let step = measure_cap(&value[limit..limit + 1], cap_px);
            if width + step > max_px {
                break;
            }
            width += step;
            limit += 1;
        }
        if limit >= value.len() {
            // The rest of the text fits on this row, so this is the last one.
            break;
        }
        let mut candidate = limit;
        while candidate > row_start && value[candidate - 1] != b' ' {
            candidate -= 1;
        }
        if candidate <= row_start {
            // WHY the hard break: a row of text with no space in it has
            // nowhere better to go.
            starts[count] = limit as u8;
            count += 1;
            row_start = limit;
            continue;
        }
        // WHY the space is left out of the row and the next row starts after
        // it: a row that ends with a space and a row that begins with one
        // both look like the text has a stray character in it.
        starts[count] = candidate as u8;
        count += 1;
        row_start = candidate;
    }
    (starts, count as u8)
}

/// What a card in the body is: a panel of measurements, or a message.
pub const CARD_PANEL: u8 = 0;
/// A message the platform said: a bubble on the left.
pub const CARD_BUBBLE_THEM: u8 = 1;
/// A message the person said: a bubble on the right.
pub const CARD_BUBBLE_ME: u8 = 2;

/// One card of the body.
#[derive(Clone, Copy)]
pub struct Card {
    /// The card's heading.
    pub title: [u8; TITLE_MAX],
    /// The heading's length.
    pub title_len: u8,
    /// How the heading is coloured.
    pub title_tone: Tone,
    /// The first line of the card in the body's line array.
    pub line_start: u8,
    /// How many lines the card holds.
    pub line_count: u8,
    /// The card's top, in body coordinates.
    pub y: u16,
    /// The card's height.
    pub height: u16,
    /// Whether the card is a panel or a message bubble.
    pub kind: u8,
    /// The bubble's width, in pixels.
    ///
    /// WHY a bubble carries its own width and a panel does not: a panel is
    /// the full content column, and a bubble is as wide as the words in it.
    /// A bubble drawn at the full width is a paragraph in a box, which is
    /// what a message is not.
    pub width: u16,
}

impl Card {
    const fn empty() -> Self {
        Self {
            title: [0; TITLE_MAX],
            title_len: 0,
            title_tone: Tone::Muted,
            line_start: 0,
            line_count: 0,
            y: 0,
            height: 0,
            kind: CARD_PANEL,
            width: 0,
        }
    }
}

/// What a bottom action bar button does.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Re-measure the device.
    Refresh,
    /// Ask the platform to install the selected provider's package.
    Install,
    /// Ask the platform to remove the selected provider's package.
    Remove,
    /// Run the self-test.
    RunTest,
    /// Run bounded agent tools.
    RunTools,
    /// Send the prompt to whatever model the device has.
    Send,
    /// Stop/cancel active streaming token generation.
    CancelStream,
    /// Empty the conversation and the prompt.
    ClearChat,
    /// Go to the conversation.
    OpenChat,
}

/// A bottom action bar button.
#[derive(Clone, Copy)]
pub struct ActionButton {
    /// The button's label.
    pub label: &'static str,
    /// What it does.
    pub action: Action,
    /// Whether it can be pressed.
    pub enabled: bool,
}

// -----------------------------------------------------------------------------
// The app
// -----------------------------------------------------------------------------

/// The agent manager's state.
pub struct AgentManagerApp {
    /// The surface on screen.
    pub surface: Surface,
    /// What was measured about the device.
    pub device: DeviceState,
    /// The capability grants the platform reported.
    pub grants: [CapabilityGrant; CAPABILITY_COUNT],
    /// The search configuration the platform holds.
    pub search: SearchConfiguration,
    /// The self-test's last run.
    pub self_test: SelfTest,
    /// Which provider is selected, by index into the catalogue.
    pub selected: usize,
    /// The last action's result, in the platform's own terms.
    pub action_log: [u8; ACTION_LOG_MAX],
    /// The result's length.
    pub action_log_len: usize,
    /// How the result is coloured.
    pub action_tone: Tone,
    /// The body's scroll position.
    pub scroll: ScrollState,
    /// The body's cards.
    pub cards: [Card; MAX_CARDS],
    /// How many cards the body holds.
    pub card_count: u8,
    /// The body's lines.
    pub lines: [Line; MAX_LINES],
    /// How many lines the body holds.
    pub line_count: u8,
    /// The body's total height, in body coordinates.
    pub content_height: u16,
    /// The conversation and the prompt being written.
    pub chat: ChatSession,
    /// Whether the overflow sheet is open.
    pub sheet_open: bool,
    /// Storage capacity in bytes from the microkernel driver.
    pub storage_bytes: Option<u64>,
    /// The platform's answer about this app's authority to run agent tools.
    pub tool_authority: ToolAuthority,
    /// What the platform said the last time this app asked for that authority.
    pub tool_consent: ConsentOutcome,
    /// Bounded agent tool execution outcomes.
    pub agent_tools: [AgentToolOutcome; 5],
    /// Physical memory diagnostics from microkernel.
    pub diagnostics: RuntimeDiagnostics,
    /// Measured IPC latency in ticks.
    pub ipc_latency_ticks: u64,
    /// The platform band's clock lane, as the RTC answered it.
    ///
    /// WHY it is cached on the app: the read is a syscall and the band is
    /// rasterised per pixel, so a band that asked per pixel would ask
    /// thousands of times a frame. The loop reads it once per tick and hands
    /// the bytes down.
    pub clock: [u8; CLOCK_LEN],
    /// Whether the RTC answered at all. When it did not, the lane stays blank.
    pub clock_ok: bool,
}

impl AgentManagerApp {
    /// Builds the app and takes the first measurement.
    pub fn new() -> Self {
        let pref_idx = load_saved_provider().unwrap_or(0);
        let authority = ToolAuthority::probe();
        let mut app = Self {
            surface: Surface::Models,
            device: DeviceState::measure(),
            grants: probe_capability_grants(),
            search: SearchConfiguration::measure(),
            self_test: SelfTest::run(),
            selected: pref_idx,
            action_log: [0; ACTION_LOG_MAX],
            action_log_len: 0,
            action_tone: Tone::Muted,
            scroll: ScrollState::new(0, BODY_H, 0),
            cards: [Card::empty(); MAX_CARDS],
            card_count: 0,
            lines: [Line::empty(); MAX_LINES],
            line_count: 0,
            content_height: 0,
            chat: ChatSession::new(),
            sheet_open: false,
            storage_bytes: std::storage_query().ok(),
            tool_authority: authority,
            tool_consent: ConsentOutcome::Refused,
            agent_tools: [
                AgentToolOutcome::execute_with_authority(AgentTool::Calculator, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::CalendarClock, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::StorageStats, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::SettingsQuery, &authority),
                AgentToolOutcome::execute_with_authority(AgentTool::CapabilityCheck, &authority),
            ],
            diagnostics: RuntimeDiagnostics::measure(),
            ipc_latency_ticks: 0,
            clock: [b' '; CLOCK_LEN],
            clock_ok: false,
        };
        app.set_action(b"Measurements taken from the platform.", Tone::Muted);
        app.rebuild();
        app
    }

    /// Records the result of the last action.
    pub fn set_action(&mut self, message: &[u8], tone: Tone) {
        let len = message.len().min(ACTION_LOG_MAX);
        self.action_log[..len].copy_from_slice(&message[..len]);
        self.action_log_len = len;
        self.action_tone = tone;
    }

    /// The provider the app has selected.
    pub fn selected_provider(&self) -> &'static dyn Provider {
        let catalogue = providers();
        let index = self.selected.min(catalogue.len().saturating_sub(1));
        catalogue[index]
    }

    /// Re-measures everything the app reports and rebuilds the body.
    pub fn refresh(&mut self) {
        self.device = DeviceState::measure();
        self.storage_bytes = std::storage_query().ok();
        self.grants = probe_capability_grants();
        self.search = SearchConfiguration::measure();
        self.diagnostics = RuntimeDiagnostics::measure();
        let authority = ToolAuthority::probe();
        self.tool_authority = authority;
        self.agent_tools = [
            AgentToolOutcome::execute_with_authority(AgentTool::Calculator, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CalendarClock, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::StorageStats, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::SettingsQuery, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CapabilityCheck, &authority),
        ];
        self.set_action(b"Re-measured: device, grants and search configuration.", Tone::Muted);
        self.rebuild();
    }

    /// Runs the bounded agent tools under the platform's authority.
    ///
    /// WHY the app asks the platform before it runs anything: the platform is
    /// the only party that can mint the agent-task capability, and a tool that
    /// ran without it would be the app deciding on its own that it may.
    pub fn run_tools(&mut self) {
        self.tool_consent = ToolAuthority::request();
        let authority = ToolAuthority::probe();
        self.tool_authority = authority;
        self.agent_tools = [
            AgentToolOutcome::execute_with_authority(AgentTool::Calculator, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CalendarClock, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::StorageStats, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::SettingsQuery, &authority),
            AgentToolOutcome::execute_with_authority(AgentTool::CapabilityCheck, &authority),
        ];
        match self.tool_consent {
            ConsentOutcome::Granted => {
                self.set_action(b"Agent tools ran under the platform's grant.", Tone::Good);
            }
            ConsentOutcome::Pending => {
                self.set_action(
                    b"Consent requested: the platform is waiting for your decision.",
                    Tone::Muted,
                );
            }
            ConsentOutcome::Refused => {
                self.set_action(
                    b"Consent refused: the platform holds no agent task capability.",
                    Tone::Bad,
                );
            }
        }
        self.rebuild();
    }

    /// Runs the self-test again and rebuilds the body.
    pub fn run_self_test(&mut self) {
        self.self_test = SelfTest::run();
        let passed = self.self_test.rows.iter().filter(|row| row.passed).count();
        if self.self_test.all_passed {
            self.set_action(b"Self-test finished: every row passed.", Tone::Good);
        } else {
            let mut message = [0u8; 48];
            let mut n = 0;
            for byte in b"Self-test finished: " {
                message[n] = *byte;
                n += 1;
            }
            n += agent_manager::format_u32(passed as u32, &mut message[n..]);
            for byte in b" of 6 rows passed." {
                message[n] = *byte;
                n += 1;
            }
            self.set_action(&message[..n], Tone::Bad);
        }
        self.rebuild();
    }

    /// Asks the platform to install the selected provider's package.
    ///
    /// WHY the platform's own call and not a local flag: the app has no
    /// authority of its own here. The kernel checks the package's signature,
    /// the caller's filesystem-write capability and the consent, and the
    /// answer it gives is the answer the screen shows.
    pub fn install_selected(&mut self) {
        let provider = self.selected_provider();
        let outcome = transact_package_install(provider, &self.device, std::package_install);
        match outcome {
            PackageInstallOutcome::Installed => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"INSTALL: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" installed successfully." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Good);
            }
            PackageInstallOutcome::AlreadyInstalled => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"INSTALL: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" is already installed." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Plain);
            }
            PackageInstallOutcome::RolledBack(code) => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"INSTALL: failed for " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" (rolled back, error " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageInstallOutcome::PermissionDenied => {
                self.set_action(
                    b"INSTALL: permission denied (requires FileSystemWrite capability).",
                    Tone::Unavailable,
                );
            }
            PackageInstallOutcome::NoPackage => {
                self.set_action(
                    b"INSTALL: this provider is not delivered as a package, so there is nothing to install.",
                    Tone::Unavailable,
                );
            }
        }
        self.device = DeviceState::measure();
        self.rebuild();
    }

    /// Asks the platform to remove the selected provider's package.
    pub fn remove_selected(&mut self) {
        let provider = self.selected_provider();
        let outcome = transact_package_remove(
            provider,
            &self.device,
            self.selected,
            std::package_uninstall,
            std::fs_unlink,
        );
        match outcome {
            PackageRemoveOutcome::Uninstalled => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" uninstalled." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Good);
            }
            PackageRemoveOutcome::InUse => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: refused - " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" is currently in use." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::NotFound => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" was not found in registry." { msg[n] = b; n += 1; }
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::ModelFileUnlinked => {
                self.set_action(b"REMOVE: /data/models/local_model.gguf unlinked.", Tone::Good);
            }
            PackageRemoveOutcome::ReadOnlyFileSystem(code) => {
                let mut msg = [0u8; 64];
                let mut n = 0;
                for &b in b"REMOVE: /system/models is read-only (code " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::ModelFileFailed(code) => {
                let mut msg = [0u8; 64];
                let mut n = 0;
                for &b in b"REMOVE: unlinking model file failed (code " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
            PackageRemoveOutcome::PermissionDenied => {
                self.set_action(
                    b"REMOVE: permission denied (requires FileSystemWrite capability).",
                    Tone::Unavailable,
                );
            }
            PackageRemoveOutcome::NotRemovable => {
                self.set_action(
                    b"REMOVE: this provider is not delivered as a package, so there is nothing to remove.",
                    Tone::Unavailable,
                );
            }
            PackageRemoveOutcome::Failed(code) => {
                let mut msg = [0u8; 96];
                let mut n = 0;
                for &b in b"REMOVE: failed for " { msg[n] = b; n += 1; }
                if let Some(package_id) = provider.package_id() {
                    for &b in package_id.as_bytes() { msg[n] = b; n += 1; }
                }
                for &b in b" (error " { msg[n] = b; n += 1; }
                n += format_u32(code.unsigned_abs() as u32, &mut msg[n..]);
                msg[n] = b')'; n += 1;
                self.set_action(&msg[..n], Tone::Unavailable);
            }
        }
        self.device = DeviceState::measure();
        self.rebuild();
    }

    /// Selects a provider and rebuilds the body.
    pub fn select_provider(&mut self, index: usize) {
        let catalogue = providers();
        if index >= catalogue.len() {
            return;
        }
        self.selected = index;
        save_preferred_provider(index);
        let provider = catalogue[index];
        let mut message = [0u8; ACTION_LOG_MAX];
        let mut n = 0;
        for byte in b"Selected: " {
            message[n] = *byte;
            n += 1;
        }
        let name = provider.display_name().as_bytes();
        let take = name.len().min(ACTION_LOG_MAX - n);
        message[n..n + take].copy_from_slice(&name[..take]);
        n += take;
        self.set_action(&message[..n], Tone::Accent);
        self.rebuild();
    }

    // -------------------------------------------------------------------------
    // Building the body
    // -------------------------------------------------------------------------

    /// The bottom of the body's band on the current surface.
    ///
    /// WHY the band moves: the chat surface carries a composer, and a
    /// composer that scrolls away with the conversation is a composer that
    /// cannot be reached while the answer is being read.
    pub fn body_bottom(&self) -> usize {
        if self.surface == Surface::Chat {
            ACTION_TOP - COMPOSER_H
        } else {
            ACTION_TOP
        }
    }

    /// The height of the body's band on the current surface.
    pub fn body_h(&self) -> usize {
        self.body_bottom().saturating_sub(BODY_TOP)
    }

    /// Rebuilds the body for the current surface.
    pub fn rebuild(&mut self) {
        self.lines = [Line::empty(); MAX_LINES];
        self.cards = [Card::empty(); MAX_CARDS];
        self.line_count = 0;
        self.card_count = 0;

        match self.surface {
            Surface::Models => self.build_models(),
            Surface::Access => self.build_access(),
            Surface::Runtime => self.build_runtime(),
            Surface::Search => self.build_search(),
            Surface::Test => self.build_test(),
            Surface::Chat => self.build_chat(),
        }

        self.layout();
    }

    /// Starts a card.
    fn begin_card(&mut self, title: &[u8], tone: Tone) {
        let index = self.card_count as usize;
        if index >= MAX_CARDS {
            return;
        }
        let mut card = Card::empty();
        let len = title.len().min(TITLE_MAX);
        card.title[..len].copy_from_slice(&title[..len]);
        card.title_len = len as u8;
        card.title_tone = tone;
        card.line_start = self.line_count;
        card.kind = CARD_PANEL;
        self.cards[index] = card;
        self.card_count += 1;
    }

    /// Starts a message bubble.
    ///
    /// WHY a bubble has no heading: the speaker is shown by which side the
    /// bubble is on, and a heading above it would say the same thing twice.
    fn begin_bubble(&mut self, kind: u8) {
        let index = self.card_count as usize;
        if index >= MAX_CARDS {
            return;
        }
        let mut card = Card::empty();
        card.line_start = self.line_count;
        card.kind = kind;
        self.cards[index] = card;
        self.card_count += 1;
    }

    /// Appends a line with a label and a value.
    fn add_line(&mut self, label: &[u8], value: &[u8], tone: Tone) {
        self.push_line(label, value, tone, false, -1, false);
    }

    /// Appends a line whose value is a status chip.
    fn add_chip(&mut self, label: &[u8], value: &[u8], tone: Tone) {
        self.push_line(label, value, tone, false, -1, true);
    }

    /// Appends a line that can be tapped to select a provider and whose
    /// value is a status chip.
    fn add_selectable_chip(&mut self, label: &[u8], value: &[u8], tone: Tone, action: i8) {
        self.push_line(label, value, tone, false, action, true);
    }

    /// Appends a line that runs the full width of the card.
    fn add_wide(&mut self, value: &[u8], tone: Tone) {
        self.push_line(b"", value, tone, true, -1, false);
    }

    fn push_line(
        &mut self,
        label: &[u8],
        value: &[u8],
        tone: Tone,
        wide: bool,
        action: i8,
        chip: bool,
    ) {
        let index = self.line_count as usize;
        if index >= MAX_LINES {
            return;
        }
        let mut line = Line::empty();
        let label_len = label.len().min(LABEL_MAX);
        line.label[..label_len].copy_from_slice(&label[..label_len]);
        if label.len() > LABEL_MAX {
            // WHY an ellipsis: a cut that looks like a whole label is a label
            // that changed the meaning. This one carried the expression the
            // number beside it came from, and half an expression reads as a
            // different expression. The mark says the text continues.
            line.label[LABEL_MAX - 3..LABEL_MAX].copy_from_slice(b"...");
        }
        line.label_len = label_len as u8;
        let value_len = value.len();
        let kept = value_len.min(WIDE_MAX);
        line.value[..kept].copy_from_slice(&value[..kept]);
        if value_len > WIDE_MAX {
            // WHY an ellipsis: a wide line that does not fit is cut, and a
            // cut that looks like a whole sentence is a sentence that
            // changed the meaning. The mark says the text continues.
            line.value[WIDE_MAX - 3..WIDE_MAX].copy_from_slice(b"...");
            line.value_len = WIDE_MAX as u8;
        } else {
            line.value_len = kept as u8;
        }
        line.tone = tone;
        line.wide = wide;
        if wide {
            let (starts, count) = wrap_rows(&line.value[..line.value_len as usize], CAP_CAPTION, WIDE_W);
            line.row_starts = starts;
            line.row_count = count;
        }
        line.action = action;
        line.chip = chip;
        self.lines[index] = line;
        self.line_count += 1;

        // WHY the card's own count is bumped here: a line belongs to the card
        // that is open, and counting them anywhere else would leave a card
        // that knows its first line but not how many it has.
        if self.card_count > 0 {
            let card_index = self.card_count as usize - 1;
            self.cards[card_index].line_count += 1;
        }
    }

    /// Places the cards and measures the body's height.
    fn layout(&mut self) {
        let mut y = 0usize;
        for index in 0..self.card_count as usize {
            let card = self.cards[index];
            let mut height;
            let mut width = CONTENT_W;

            if card.kind == CARD_PANEL {
                height = LINES_TOP;
                for line_index in 0..card.line_count as usize {
                    let line = self.lines[card.line_start as usize + line_index];
                    height += line.rows() * ROW_H;
                }
                height += CARD_PAD_BOTTOM;
            } else {
                // WHY a bubble is measured by its longest row: a bubble that
                // is the full width of the column is a paragraph in a box,
                // and a bubble that is wider than its words is a bubble with
                // a hole in it. The width is the words plus the padding
                // they sit inside.
                let mut widest = 0usize;
                let mut rows_total = 0usize;
                for line_index in 0..card.line_count as usize {
                    let line = self.lines[card.line_start as usize + line_index];
                    rows_total += line.rows();
                    for row in 0..line.rows() {
                        let (start, end) = line.row_range(row);
                        widest = widest.max(measure_cap(&line.value[start..end], CAP_CAPTION));
                    }
                }
                width = (widest + 2 * BUBBLE_PAD).min(CONTENT_W);
                height = 2 * BUBBLE_PAD + rows_total * ROW_H;
            }

            self.cards[index].y = y as u16;
            self.cards[index].height = height as u16;
            self.cards[index].width = width as u16;
            y += height + CARD_GAP;
        }
        // WHY the band under the last card: the floating button covers the
        // bottom-right of the body, and a value that sits under it is a
        // value a person cannot read. The band is empty space the last card
        // can be scrolled past, so the button never sits on a value.
        //
        // WHY the chat surface has no band: it has no floating button, and a
        // composer that scrolls further than it needs to is a composer that
        // moves under the thumb for no reason.
        let reserved = if self.surface == Surface::Chat {
            0
        } else {
            FAB_R * 2 + FAB_MARGIN
        };
        let content = y.saturating_sub(CARD_GAP) + reserved;
        self.content_height = content as u16;
        self.scroll = ScrollState::new(content, self.body_h(), self.scroll.offset);
    }

    // -------------------------------------------------------------------------
    // The surfaces
    // -------------------------------------------------------------------------

    /// Builds the model management surface.
    fn build_models(&mut self) {
        let models = self.device.models;
        self.begin_card(b"Device model state", Tone::Plain);
        self.add_line(b"Model directory", MODEL_DIRECTORY_LABEL, Tone::Plain);
        self.add_chip(
            b"Directory",
            models.status.as_str().as_bytes(),
            model_tone(models.status),
        );
        let mut count = [0u8; 8];
        let n = format_u32(models.entries as u32, &mut count);
        self.add_line(b"Entries", &count[..n], Tone::Plain);
        if let Some(bytes) = self.storage_bytes {
            let mut storage_buf = [0u8; 24];
            let gb_int = bytes / (1024 * 1024 * 1024);
            let gb_frac = (bytes % (1024 * 1024 * 1024)) * 100 / (1024 * 1024 * 1024);
            let mut sn = format_u32(gb_int as u32, &mut storage_buf);
            storage_buf[sn] = b'.';
            sn += 1;
            if gb_frac < 10 {
                storage_buf[sn] = b'0';
                sn += 1;
            }
            let fn_len = format_u32(gb_frac as u32, &mut storage_buf[sn..]);
            sn += fn_len;
            for &b in b" GB" {
                storage_buf[sn] = b;
                sn += 1;
            }
            self.add_line(b"Disk capacity", &storage_buf[..sn], Tone::Plain);
            let mut mounted_buf = [0u8; 32];
            let mut mn = 0;
            for &b in &storage_buf[..sn] {
                mounted_buf[mn] = b;
                mn += 1;
            }
            for &b in b" MOUNTED" {
                mounted_buf[mn] = b;
                mn += 1;
            }
            self.add_chip(b"Storage link", &mounted_buf[..mn], Tone::Good);
        } else {
            self.add_line(b"Disk capacity", b"Unavailable", Tone::Muted);
            self.add_chip(b"Storage link", b"NOT LINKED", Tone::Bad);
        }
        self.add_chip(
            b"Embedding weights",
            if models.has_embedding_weights {
                b"PRESENT"
            } else {
                b"ABSENT"
            },
            if models.has_embedding_weights {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        self.add_chip(
            b"Generative weights",
            if models.has_generative_weights {
                b"PRESENT"
            } else {
                b"ABSENT"
            },
            if models.has_generative_weights {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        let embed_stage = models.embedding_stage(&self.device.services);
        self.add_chip(
            b"Embedding stage",
            embed_stage.map(|s| s.as_str().as_bytes()).unwrap_or(b"ABSENT"),
            if embed_stage.is_some() { Tone::Good } else { Tone::Muted },
        );
        if models.embedding_size_kb > 0 {
            let mut sbuf = [0u8; 24];
            let mut sn = format_u32(models.embedding_size_kb as u32, &mut sbuf);
            for &b in b" KB" { sbuf[sn] = b; sn += 1; }
            self.add_line(b"Embedding size", &sbuf[..sn], Tone::Plain);
        }

        let gen_stage = models.generative_stage(&self.device.services);
        self.add_chip(
            b"Generative stage",
            gen_stage.map(|s| s.as_str().as_bytes()).unwrap_or(b"ABSENT"),
            if gen_stage.is_some() { Tone::Good } else { Tone::Muted },
        );
        if models.generative_size_kb > 0 {
            let mut sbuf = [0u8; 24];
            let mut sn = format_u32(models.generative_size_kb as u32, &mut sbuf);
            for &b in b" KB" { sbuf[sn] = b; sn += 1; }
            self.add_line(b"Generative size", &sbuf[..sn], Tone::Plain);
        }
        if models.status == ModelProbeStatus::Absent {
            self.add_wide(
                b"The platform has no model directory on this device, so the device holds no model weights.",
                Tone::Muted,
            );
        } else if models.status == ModelProbeStatus::Refused {
            self.add_wide(
                b"The platform refused the listing, so this app cannot say what the device holds.",
                Tone::Unavailable,
            );
        } else if models.status == ModelProbeStatus::NoAuthority {
            self.add_wide(
                b"This app holds no filesystem-read capability, so it cannot list the model directory.",
                Tone::Unavailable,
            );
        }

        self.begin_card(b"Provider catalogue", Tone::Accent);
        for (index, provider) in providers().iter().enumerate() {
            let state = provider.install_state(&self.device);
            self.add_selectable_chip(
                provider.display_name().as_bytes(),
                state.as_str().as_bytes(),
                install_tone(state),
                index as i8,
            );
            self.add_wide(provider.capabilities().role().as_bytes(), Tone::Muted);
        }
        self.add_wide(
            b"The install state is a measurement of this device, not a vendor's plan.",
            Tone::Muted,
        );

        let selected = self.selected_provider();
        self.begin_card(b"Selected provider", Tone::Plain);
        self.add_line(b"Provider", selected.display_name().as_bytes(), Tone::Accent);
        self.add_line(b"Identifier", selected.id().as_bytes(), Tone::Plain);
        let state = selected.install_state(&self.device);
        self.add_chip(b"Install state", state.as_str().as_bytes(), install_tone(state));
        let binding = selected.runtime_binding(&self.device);
        self.add_chip(
            b"Runtime",
            binding.as_str().as_bytes(),
            if binding.is_bound() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        if let RuntimeBinding::Unbound { reason } = binding {
            self.add_wide(reason.as_bytes(), Tone::Muted);
        }
        self.add_line(
            b"Package",
            selected.package_id().unwrap_or("none").as_bytes(),
            Tone::Plain,
        );
        if let Some(version) = selected.package_version(&self.device) {
            let mut ver_buf = [0u8; 32];
            let mut n = 0;
            n += format_u32(version.0 as u32, &mut ver_buf[n..]);
            ver_buf[n] = b'.'; n += 1;
            n += format_u32(version.1 as u32, &mut ver_buf[n..]);
            ver_buf[n] = b'.'; n += 1;
            n += format_u32(version.2 as u32, &mut ver_buf[n..]);
            self.add_line(b"Pkg version", &ver_buf[..n], Tone::Plain);
        }
        if let Some(pkg_state) = selected.package_lifecycle_state(&self.device) {
            self.add_chip(
                b"Pkg state",
                pkg_state.as_str().as_bytes(),
                if pkg_state == RegistryPackageState::Active {
                    Tone::Good
                } else {
                    Tone::Unavailable
                },
            );
        }

    }

    /// Builds the permissions surface.
    fn build_access(&mut self) {
        // WHY the grants are copied out: the builder mutates the body while
        // it walks the list, and the list belongs to the app.
        let grants = self.grants;
        self.begin_card(b"Capability grants", Tone::Plain);
        let granted = granted_capability_count(&grants);
        let mut summary = [0u8; 24];
        let mut n = 0;
        n += format_u32(granted as u32, &mut summary[n..]);
        for byte in b" of " {
            summary[n] = *byte;
            n += 1;
        }
        n += format_u32(CAPABILITY_COUNT as u32, &mut summary[n..]);
        for byte in b" granted" {
            summary[n] = *byte;
            n += 1;
        }
        self.add_chip(b"Granted", &summary[..n], Tone::Plain);
        for grant in grants.iter() {
            self.add_chip(
                grant.name.as_bytes(),
                if grant.granted {
                    b"GRANTED"
                } else {
                    b"NOT GRANTED"
                },
                if grant.granted {
                    Tone::Good
                } else {
                    Tone::Bad
                },
            );
        }

        self.begin_card(b"What the grants allow", Tone::Plain);
        let mut any = false;
        for grant in grants.iter() {
            if grant.granted {
                self.add_wide(grant.allows.as_bytes(), Tone::Plain);
                any = true;
            }
        }
        if !any {
            self.add_wide(
                b"The platform granted this app no capability at all.",
                Tone::Unavailable,
            );
        }
        self.add_wide(
            b"GRANTED means the platform's capability table holds that type for this app. It is asked of the kernel, not declared here.",
            Tone::Muted,
        );

        self.begin_card(b"What is not granted", Tone::Plain);
        let mut any = false;
        for grant in grants.iter() {
            if !grant.granted {
                self.add_wide(grant.allows.as_bytes(), Tone::Muted);
                any = true;
            }
        }
        if !any {
            self.add_wide(b"Nothing is missing: every capability asked about was granted.", Tone::Muted);
        }
    }

    /// Builds the runtime status surface.
    fn build_runtime(&mut self) {
        let services = self.device.services;
        self.begin_card(b"Service registry", Tone::Plain);
        let mut count = [0u8; 24];
        let mut n = 0;
        n += format_u32(services.count as u32, &mut count[n..]);
        for byte in b" service(s)" {
            count[n] = *byte;
            n += 1;
        }
        self.add_line(b"Registered", &count[..n], Tone::Plain);
        self.add_line(
            b"Registry query",
            services.status.as_str().as_bytes(),
            match services.status {
                ProbeStatus::Listed => Tone::Good,
                ProbeStatus::Refused => Tone::Unavailable,
            },
        );
        for entry in services.entries.iter().take(services.count as usize) {
            self.add_line(
                service_name(entry.id).as_bytes(),
                entry.state.as_str().as_bytes(),
                service_tone(entry.state),
            );
        }
        if services.count == 0 && services.status == ProbeStatus::Listed {
            self.add_wide(b"The registry answered and listed no services.", Tone::Muted);
        }

        self.begin_card(b"Inference runtime", Tone::Accent);
        let binding = services.inference_runtime_binding();
        self.add_line(
            b"Runtime",
            binding.as_str().as_bytes(),
            if binding.is_bound() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        if let RuntimeBinding::Unbound { reason } = binding {
            self.add_wide(reason.as_bytes(), Tone::Muted);
        }
        let mut highest = [0u8; 16];
        let n = format_u32(services.highest_id as u32, &mut highest);
        self.add_line(b"Highest service id", &highest[..n], Tone::Plain);
        self.add_line(b"Service ABI range", b"1..=11", Tone::Plain);
        self.add_wide(
            b"The launcher-to-service path, the authorisation and the retrieval are not wired up on this platform, so no system-wide integration is claimed.",
            Tone::Muted,
        );

        self.begin_card(b"System telemetry & diagnostics", Tone::Plain);
        if self.diagnostics.has_ram_telemetry {
            let mut buf = [0u8; 24];
            let mut n = format_u32(self.diagnostics.ram_total_kb as u32, &mut buf);
            for &b in b" KB" { buf[n] = b; n += 1; }
            self.add_line(b"RAM total", &buf[..n], Tone::Plain);

            let mut n = format_u32(self.diagnostics.ram_used_kb as u32, &mut buf);
            for &b in b" KB" { buf[n] = b; n += 1; }
            self.add_line(b"RAM used", &buf[..n], Tone::Plain);

            let mut n = format_u32(self.diagnostics.ram_free_kb as u32, &mut buf);
            for &b in b" KB" { buf[n] = b; n += 1; }
            self.add_line(b"RAM free", &buf[..n], Tone::Plain);
        } else {
            self.add_line(b"Physical RAM", b"Unavailable", Tone::Muted);
        }

        if self.ipc_latency_ticks > 0 {
            let mut lbuf = [0u8; 24];
            let mut ln = format_u32(self.ipc_latency_ticks as u32, &mut lbuf);
            for &b in b" ticks" { lbuf[ln] = b; ln += 1; }
            self.add_line(b"IPC latency", &lbuf[..ln], Tone::Plain);
        } else if self.device.services.contains_inference_runtime() {
            self.add_line(b"IPC latency", b"Active", Tone::Good);
        } else {
            self.add_line(b"IPC latency", b"Unbound: no target", Tone::Muted);
        }

        self.add_line(
            b"Generation stream",
            if self.chat.is_streaming { b"Streaming active" } else { b"Idle" },
            if self.chat.is_streaming { Tone::Accent } else { Tone::Plain },
        );
    }

    /// Builds the search configuration surface.
    fn build_search(&mut self) {
        let search = self.search;
        self.begin_card(b"Retrieval settings", Tone::Plain);
        let held = search.dimension.is_some();
        let dimension = search.dimension.unwrap_or(MATRYOSHKA_DEFAULT_DIM);
        let mut value = [0u8; VALUE_MAX];
        let mut n = format_u32(dimension as u32, &mut value);
        for byte in if held {
            &b" (platform)"[..]
        } else {
            &b" (default)"[..]
        } {
            value[n] = *byte;
            n += 1;
        }
        self.add_line(b"Vector dimension", &value[..n], Tone::Plain);

        let held = search.threshold_q15.is_some();
        let threshold = search.threshold_q15.unwrap_or(SIMILARITY_DEFAULT_Q15);
        let mut value = [0u8; VALUE_MAX];
        let mut n = format_threshold(threshold, &mut value);
        for byte in if held {
            &b" platform"[..]
        } else {
            &b" default"[..]
        } {
            if n < value.len() {
                value[n] = *byte;
                n += 1;
            }
        }
        self.add_line(b"Similarity cutoff", &value[..n], Tone::Plain);

        let held = search.max_results.is_some();
        let max_results = search.max_results.unwrap_or(MAX_RESULTS_DEFAULT);
        let mut value = [0u8; VALUE_MAX];
        let mut n = format_u32(max_results as u32, &mut value);
        for byte in if held {
            &b" (platform)"[..]
        } else {
            &b" (default)"[..]
        } {
            value[n] = *byte;
            n += 1;
        }
        self.add_line(b"Max results", &value[..n], Tone::Plain);

        self.add_line(
            b"Filesystem scan",
            if search.filesystem_scan {
                b"ON"
            } else {
                b"OFF"
            },
            if search.filesystem_scan {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        self.add_line(
            b"Settings are",
            if search.writable {
                b"writable"
            } else {
                b"read-only here"
            },
            if search.writable {
                Tone::Good
            } else {
                Tone::Muted
            },
        );
        if search.dimension.is_none() {
            self.add_wide(
                b"The platform's settings store holds no search.* key, so the values above are the agent manager's own defaults.",
                Tone::Muted,
            );
        }

        self.begin_card(b"Search sources", Tone::Plain);
        for source in search.sources.iter() {
            let mut value = [0u8; VALUE_MAX];
            let n = if source.status == ProbeStatus::Listed {
                format_u32(source.count as u32, &mut value)
            } else {
                let text = b"REFUSED";
                value[..text.len()].copy_from_slice(text);
                text.len()
            };
            self.add_line(
                source.name.as_bytes(),
                &value[..n],
                match source.status {
                    ProbeStatus::Listed => Tone::Plain,
                    ProbeStatus::Refused => Tone::Unavailable,
                },
            );
        }
        self.add_wide(
            b"A refused source is a capability the platform withheld, not an empty one.",
            Tone::Muted,
        );

        self.begin_card(b"Content layer", Tone::Accent);
        self.add_wide(
            b"The document index and the intent registry are the platform's own. The counts above are the platform's answers.",
            Tone::Muted,
        );

        self.begin_card(b"Document retrieval", Tone::Plain);
        let _ = std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_SEARCH,
            b"storage",
        );
        let mut doc_buf = [0u8; 1024];
        let mut shown = 0;
        if let Ok(n) = std::ipc_recv(&mut doc_buf) {
            let resp_hdr_len = core::mem::size_of::<std::ai::AiInferenceResponse>();
            let hit_hdr_len = core::mem::size_of::<std::ai::AiSearchHit>();
            if n >= resp_hdr_len + hit_hdr_len {
                let resp = unsafe {
                    core::ptr::read_unaligned(doc_buf.as_ptr() as *const std::ai::AiInferenceResponse)
                };
                if resp.status == std::ai::STATUS_SUCCESS && resp.output_dim > 0 {
                    let mut cursor = resp_hdr_len;
                    let hits_count = resp.output_dim as usize;
                    for _ in 0..hits_count {
                        if cursor + hit_hdr_len > n || shown >= 3 {
                            break;
                        }
                        let hit = unsafe {
                            core::ptr::read_unaligned(doc_buf.as_ptr().add(cursor) as *const std::ai::AiSearchHit)
                        };
                        cursor += hit_hdr_len;
                        let path_len = hit.path_len as usize;
                        let snip_len = hit.snippet_len as usize;
                        if cursor + path_len + snip_len > n {
                            break;
                        }
                        let path_bytes = &doc_buf[cursor..cursor + path_len];
                        cursor += path_len;
                        let snippet_bytes = &doc_buf[cursor..cursor + snip_len];
                        cursor += snip_len;

                        let path_or_snippet = if !path_bytes.is_empty() {
                            path_bytes
                        } else {
                            snippet_bytes
                        };
                        self.add_line(b"Ranked result", path_or_snippet, Tone::Plain);
                        self.add_chip(
                            b"Attribution",
                            if hit.is_neural != 0 {
                                b"[Neural RAG]"
                            } else {
                                b"[Lexical RAG]"
                            },
                            Tone::Good,
                        );
                        shown += 1;
                    }
                }
            }
        }
        if shown == 0 {
            let mut list_buf = [0u8; 512];
            match std::fs_list("/storage", &mut list_buf) {
                Ok(n) if n > 0 => {
                    let payload = &list_buf[..n.min(list_buf.len())];
                    for line in payload.split(|&b| b == b'\n') {
                        if line.is_empty() || shown >= 3 {
                            continue;
                        }
                        let mut parts = line.split(|&b| b == b'|');
                        if let Some(name) = parts.next() {
                            if !name.is_empty() {
                                self.add_line(b"Ranked result", name, Tone::Plain);
                                self.add_chip(b"Attribution", b"[Lexical RAG]", Tone::Good);
                                shown += 1;
                            }
                        }
                    }
                    if shown == 0 {
                        self.add_wide(b"Zero indexed documents reported by content service.", Tone::Muted);
                    }
                }
                _ => {
                    self.add_wide(b"Zero indexed documents reported by content service.", Tone::Muted);
                }
            }
        }
    }

    /// Builds the chat surface.
    ///
    /// WHY the service state comes first: the conversation is the
    /// feature, but the answer to "why is there no reply" is the service
    /// state, and somebody about to type deserves to know before they
    /// type rather than after.
    fn build_chat(&mut self) {
        let availability = resolve_inference_service(&self.device.services);
        self.begin_card(b"Model service", Tone::Plain);
        self.add_chip(
            b"State",
            if availability.is_available() {
                b"running"
            } else {
                b"not available"
            },
            if availability.is_available() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        self.add_wide(availability.reason().as_bytes(), Tone::Muted);

        if self.chat.is_empty() {
            // WHY an empty conversation still says something: a screen that
            // is blank tells the reader nothing, and the honest thing to
            // say is what the surface is for.
            self.begin_bubble(CARD_BUBBLE_THEM);
            self.add_wide(
                b"No messages yet. Type a prompt below and press the send button.",
                Tone::Plain,
            );
        } else {
            for position in 0..self.chat.len() {
                // WHY the message is copied out: the builder mutates the
                // body while it walks the transcript, and the transcript
                // belongs to the app.
                let Some(message) = self.chat.message_at(position).copied() else {
                    break;
                };
                let role = message.role;
                let len = message.text_len as usize;
                // WHY the speaker is shown by the side of the bubble: a
                // label above every message repeats what the layout already
                // says, and the design the showcases describe puts the
                // person on the right and the platform on the left.
                let kind = match role {
                    ChatRole::User => CARD_BUBBLE_ME,
                    _ => CARD_BUBBLE_THEM,
                };
                self.begin_bubble(kind);
                self.add_wide(&message.text[..len], chat_role_ink(role));
            }
        }
    }

    /// Sends the prompt to whatever model the device has, starting live streaming.
    ///
    /// WHY the prompt is recorded before the answer is sought: the
    /// exchange is the user's, and it belongs in the transcript whether
    /// or not a model was there to answer it. A conversation that
    /// silently swallowed the prompt would leave the user unable to say
    /// what they had asked.
    pub fn send_prompt(&mut self) {
        if self.chat.prompt_is_empty() {
            self.set_action(b"Nothing to send.", Tone::Muted);
            return;
        }
        let mut prompt = [0u8; CHAT_PROMPT_MAX];
        let len = self.chat.prompt_text().len().min(CHAT_PROMPT_MAX);
        prompt[..len].copy_from_slice(&self.chat.prompt_text()[..len]);
        self.chat.push(ChatRole::User, &prompt[..len]);
        self.chat.clear_prompt();

        let availability = resolve_inference_service(&self.device.services);
        if !availability.is_available() {
            // WHY the reason is the reply: there is no model to answer, and
            // the honest answer to a prompt is the reason there is no
            // answer. The platform's own words, not a sentence invented
            // here.
            self.chat
                .push(ChatRole::Notice, availability.reason().as_bytes());
            self.set_action(availability.reason().as_bytes(), Tone::Unavailable);
            self.rebuild();
            return;
        } else {
            self.set_action(b"Streaming generation active...", Tone::Accent);
        }

        let t0 = std::get_ticks().unwrap_or(0);
        let _ = std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_GENERATE,
            &prompt[..len],
        );
        let t1 = std::get_ticks().unwrap_or(t0);
        self.ipc_latency_ticks = t1.saturating_sub(t0);

        // Trigger a streaming generation step: query HostBridge or simulated generative model,
        // appending generated token chunks to the transcript buffer incrementally.
        self.chat.start_stream(&prompt[..len]);
        self.step_streaming();
        self.rebuild();
    }

    /// Cancels active streaming token generation via IPC to SERVICE_INFERENCE.
    pub fn cancel_streaming(&mut self) {
        let _ = std::ipc_send(
            std::ai::SERVICE_INFERENCE_TASK_ID,
            std::ai::MSG_TYPE_CANCEL,
            b"CANC",
        );
        self.chat.finish_stream();
        self.chat.push(ChatRole::Notice, b"[Generation cancelled by user]");
        self.set_action(b"Streaming cancelled.", Tone::Plain);
        self.rebuild();
    }

    /// Advances one step of live streaming token generation.
    ///
    /// Returns true if the transcript buffer was updated and the screen
    /// needs to be repainted.
    pub fn step_streaming(&mut self) -> bool {
        if !self.chat.is_streaming {
            return false;
        }
        let step = self.chat.streaming_step as usize;
        let prompt_bytes = self.chat.stream_prompt_text();

        // 1. Try polling real IPC from aid
        let mut rx_buf = [0u8; 1024];
        if let Ok(n) = std::ipc_recv(&mut rx_buf) {
            let hdr_size = core::mem::size_of::<std::ai::AiInferenceResponse>();
            if n >= hdr_size {
                let resp = unsafe {
                    core::ptr::read_unaligned(rx_buf.as_ptr() as *const std::ai::AiInferenceResponse)
                };
                let payload = &rx_buf[hdr_size..n];
                if resp.status == 0 {
                    if step == 0 {
                        self.chat.push(ChatRole::Assistant, payload);
                    } else {
                        self.chat.append_chunk(payload);
                    }
                    self.chat.finish_stream();
                    self.set_action(b"Streaming generation complete.", Tone::Plain);
                    self.rebuild();
                    return true;
                } else {
                    self.chat.finish_stream();
                    self.chat.push(ChatRole::Notice, payload);
                    self.set_action(payload, Tone::Unavailable);
                    self.rebuild();
                    return true;
                }
            }
        }

        let chunk: StreamChunk = match self.selected {
            0 => StreamChunk::Unavailable(
                "EmbeddingGemma 2 is an embedding model and cannot generate conversational text.",
            ),
            2 => LOCAL_GENERATIVE.stream_chunk(prompt_bytes, step),
            _ => HOST_BRIDGE.stream_chunk(prompt_bytes, step),
        };
        match chunk {
            StreamChunk::Chunk(chunk_bytes) => {
                if step == 0 {
                    self.chat.push(ChatRole::Assistant, chunk_bytes);
                } else {
                    self.chat.append_chunk(chunk_bytes);
                }
                self.chat.streaming_step += 1;
                self.set_action(b"Streaming live tokens...", Tone::Accent);
                self.rebuild();
                true
            }
            StreamChunk::Unavailable(reason) => {
                self.chat.finish_stream();
                self.chat.push(ChatRole::Notice, reason.as_bytes());
                self.set_action(reason.as_bytes(), Tone::Unavailable);
                self.rebuild();
                true
            }
            StreamChunk::Done => {
                self.chat.finish_stream();
                self.set_action(b"Streaming generation complete.", Tone::Plain);
                self.rebuild();
                true
            }
        }
    }

    /// Empties the conversation and the prompt.
    pub fn clear_chat(&mut self) {
        if self.chat.is_streaming {
            self.cancel_streaming();
        }
        self.chat = ChatSession::new();
        self.set_action(b"Conversation cleared.", Tone::Muted);
        self.rebuild();
    }

    /// Builds the self-test surface.
    fn build_test(&mut self) {
        // WHY the rows are copied out: the builder mutates the body while it
        // walks the rows, and the rows belong to the app.
        let test = self.self_test;
        self.begin_card(b"Self-test", Tone::Accent);
        let passed = test.rows.iter().filter(|row| row.passed).count();
        let mut summary = [0u8; 24];
        let mut n = 0;
        n += format_u32(passed as u32, &mut summary[n..]);
        for byte in b" of " {
            summary[n] = *byte;
            n += 1;
        }
        n += format_u32(agent_manager::SELF_TEST_ROWS as u32, &mut summary[n..]);
        for byte in b" passed" {
            summary[n] = *byte;
            n += 1;
        }
        self.add_chip(
            b"Overall",
            &summary[..n],
            if test.all_passed {
                Tone::Good
            } else {
                Tone::Bad
            },
        );
        for row in test.rows.iter() {
            self.add_chip(
                row.label.as_bytes(),
                if row.passed { b"PASS" } else { b"FAIL" },
                if row.passed { Tone::Good } else { Tone::Bad },
            );
            self.add_wide(row.detail_str().as_bytes(), Tone::Muted);
        }
        self.add_wide(
            b"Every row above holds the value the platform's own function returned.",
            Tone::Muted,
        );

        self.begin_card(b"Agent tools (bounded runtime)", Tone::Plain);
        // WHY the chip reports the platform's answer and not the app's own
        // setting: the app has no setting to report. What it holds is the
        // kernel's answer, and a chip that said "active" while the kernel
        // held nothing would be a claim the screen cannot back up.
        self.add_chip(
            b"Tool authority",
            if self.tool_authority.is_granted() {
                b"[Granted]"
            } else {
                b"[Not held]"
            },
            if self.tool_authority.is_granted() {
                Tone::Good
            } else {
                Tone::Unavailable
            },
        );
        for i in 0..self.agent_tools.len() {
            let outcome = self.agent_tools[i];
            self.add_chip(
                outcome.tool.name().as_bytes(),
                if outcome.ok { b"OK" } else { b"UNAVAILABLE" },
                if outcome.ok { Tone::Good } else { Tone::Unavailable },
            );
            self.add_line(outcome.tool.description().as_bytes(), outcome.text().as_bytes(), Tone::Plain);
        }
        self.add_wide(
            b"Tools run bounded execution calling real platform APIs with zero ambient authority.",
            Tone::Muted,
        );
    }

    /// The bottom action bar's buttons for the current surface.
    pub fn actions(&self) -> [ActionButton; 3] {
        let package = self.selected_provider().package_id();
        match self.surface {
            Surface::Models => [
                ActionButton {
                    label: "REFRESH",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "INSTALL",
                    action: Action::Install,
                    enabled: package.is_some(),
                },
                ActionButton {
                    label: "REMOVE",
                    action: Action::Remove,
                    enabled: package.is_some(),
                },
            ],
            Surface::Access => [
                ActionButton {
                    label: "RE-READ GRANTS",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "REFRESH",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Runtime => [
                ActionButton {
                    label: "RE-READ REGISTRY",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Search => [
                ActionButton {
                    label: "RE-READ CONFIG",
                    action: Action::Refresh,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Test => [
                ActionButton {
                    label: "RUN SELF-TEST",
                    action: Action::RunTest,
                    enabled: true,
                },
                ActionButton {
                    label: "RUN TOOLS",
                    action: Action::RunTools,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
            Surface::Chat => [
                if self.chat.is_streaming {
                    ActionButton {
                        label: "STOP",
                        action: Action::CancelStream,
                        enabled: true,
                    }
                } else {
                    ActionButton {
                        label: "SEND",
                        action: Action::Send,
                        // WHY the button is live even when there is no model:
                        // pressing it is how the user finds out, and the
                        // answer is the platform's reason rather than a
                        // button that does nothing.
                        enabled: true,
                    }
                },
                ActionButton {
                    label: "CLEAR",
                    action: Action::ClearChat,
                    enabled: true,
                },
                ActionButton {
                    label: "",
                    action: Action::Refresh,
                    enabled: false,
                },
            ],
        }
    }
}

/// Moves the body with the keyboard.
///
/// WHY this exists: a body that can only be scrolled with a finger is a
/// body that cannot be scrolled at all on a device without one, and the
/// keys that move a list everywhere else on the platform moved nothing
/// here. The step is a quarter of the body, so a key press moves the list
/// a visible amount without running off the end of it.
fn handle_scroll_key(app: &mut AgentManagerApp, code: u32) -> bool {
    use std::input_key_code;
    let step = (app.body_h() / 4).max(1);
    match code {
        input_key_code::ARROW_DOWN => {
            let next = app.scroll.offset + step;
            app.scroll.offset = next.min(app.scroll.max_offset());
        }
        input_key_code::ARROW_UP => {
            app.scroll.offset = app.scroll.offset.saturating_sub(step);
        }
        input_key_code::PAGE_DOWN => {
            let next = app.scroll.offset + app.body_h();
            app.scroll.offset = next.min(app.scroll.max_offset());
        }
        input_key_code::PAGE_UP => {
            app.scroll.offset = app.scroll.offset.saturating_sub(app.body_h());
        }
        input_key_code::HOME => {
            app.scroll.offset = 0;
        }
        input_key_code::END => {
            app.scroll.offset = app.scroll.max_offset();
        }
        _ => return false,
    }
    true
}

/// What a key press did to the composer.
enum ChatKey {
    /// The composer changed, so the body has to be rebuilt.
    Edited,
    /// The key was not for the composer.
    Ignored,
}

/// Routes a key press to the composer.
///
/// WHY the printable range is ASCII 32..126: that is the whole of the
/// font the platform ships, so a key outside it is a character the
/// screen cannot draw. Accepting one would put a glyph in the prompt
/// that the transcript could never show again.
///
/// WHY the composer refuses a character it cannot hold rather than
/// dropping it: a composer that silently loses the last character typed
/// is a composer that lies about what was written.
fn handle_chat_key(app: &mut AgentManagerApp, code: u32) -> ChatKey {
    use std::input_key_code;
    match code {
        input_key_code::ENTER | 13 => {
            if app.chat.is_streaming {
                app.cancel_streaming();
            } else {
                app.send_prompt();
            }
            ChatKey::Edited
        }
        input_key_code::BACKSPACE | 8 => {
            app.chat.pop_prompt_char();
            app.rebuild();
            ChatKey::Edited
        }
        32..=126 => {
            if app.chat.push_prompt_char(code as u8) {
                app.rebuild();
                ChatKey::Edited
            } else {
                ChatKey::Ignored
            }
        }
        _ => ChatKey::Ignored,
    }
}

/// The model directory, as the screen spells it.
const MODEL_DIRECTORY_LABEL: &[u8] = b"/system/models";

/// The tone for a model directory probe.
fn model_tone(status: ModelProbeStatus) -> Tone {
    match status {
        ModelProbeStatus::Listed => Tone::Good,
        ModelProbeStatus::Absent => Tone::Muted,
        ModelProbeStatus::Refused | ModelProbeStatus::NoAuthority => Tone::Unavailable,
    }
}

/// The tone for a provider's install state.
fn install_tone(state: InstallState) -> Tone {
    match state {
        InstallState::Installed => Tone::Good,
        InstallState::NotInstalled => Tone::Muted,
        InstallState::Unavailable => Tone::Unavailable,
    }
}

/// The tone for a service's state.
fn service_tone(state: ServiceState) -> Tone {
    match state {
        ServiceState::Running => Tone::Good,
        ServiceState::Failed => Tone::Bad,
        ServiceState::Starting => Tone::Unavailable,
        ServiceState::Declared | ServiceState::Stopped => Tone::Muted,
        ServiceState::Unknown => Tone::Unavailable,
    }
}

/// Writes the platform's error code as text.
///
/// WHY the code is named and not just printed: a number on a screen tells
/// the reader nothing, and the difference between "permission denied" and
/// "no such package" is the difference between a capability the app lacks
/// and a package that does not exist.
#[allow(dead_code)]
fn write_error(out: &mut [u8], code: i64) -> usize {
    let mut n = 0;
    let reason: &[u8] = match code {
        -1 => b"permission denied",
        -3 => b"invalid arguments",
        -8 => b"no such package",
        _ => b"the platform refused the request",
    };
    if code < 0 {
        out[n] = b'-';
        n += 1;
    }
    n += format_u32(code.unsigned_abs() as u32, &mut out[n..]);
    for byte in b" (" {
        if n < out.len() {
            out[n] = *byte;
            n += 1;
        }
    }
    for byte in reason {
        if n < out.len() {
            out[n] = *byte;
            n += 1;
        }
    }
    if n < out.len() {
        out[n] = b')';
        n += 1;
    }
    n
}

/// Draws text in the platform's own face.
///
/// WHY the face is chosen by the scale: the guest's bitmap face rasterises a
/// 16x24 cell at integer scales only, so the larger rungs have to be drawn
/// with the platform's outline face or they come out as a doubled bitmap.
/// This is the same choice the platform's own component library makes, and
/// the same reason.
///
/// WHY the background is a parameter: the face composites its glyphs over
/// the colour underneath, and a card whose fill is not the colour the text
/// was given shows a rectangle behind every word.
#[inline(always)]
fn text(
    x: usize,
    y: usize,
    x0: usize,
    y0: usize,
    cap_px: u32,
    body: &[u8],
    ink: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if body.is_empty() {
        return None;
    }
    // WHY the outline face at every scale and not only the large ones: the
    // platform scanout is 720x1440, exactly the authoring space this app
    // paints, so composition is 1:1 and the compositor magnifies nothing.
    // The outline face is still preferred because it rasterises the real
    // outline at the requested cap height instead of magnifying the 16x24
    // bitmap fallback face. Most of the text on this screen is a small run,
    // and the outline face is a real outline rather than a scaled bitmap,
    // so it stays sharp at any size.
    //
    // WHY the bitmap face is still here: a run the outline face refuses
    // -- a character the typeface has no outline for -- has to be drawn
    // somehow, and the bitmap face is what it is drawn with.
    let em = std::design::outline_text::em_for_cap_height(cap_px);
    let (ascent, _) = std::design::typeface::line_metrics(em);
    let baseline = y0 + ascent as usize;
    if let Some(run) = std::design::outline_text::OutlineRun::place_styled(x0, baseline, em, body, bold) {
        let (bx, by, bw, bh) = run.bounds();
        if x >= bx && x < bx + bw && y >= by && y < by + bh {
            return run.pixel(x, y, ink, bg);
        }
        return None;
    }
    // WHY the fallback is the nearest whole scale: the bitmap face draws
    // in whole cells, and a run the outline face refuses -- a character the
    // typeface has no outline for -- has to be drawn at some size. It is
    // the nearest one the face has.
    let fallback = (std::design::outline_text::em_for_cap_height(cap_px) as usize / font::GLYPH_H)
        .max(1);
    font::test_text_aa(x, y, x0, y0, fallback, body, ink, bg, bold)
}

/// The width of `body` at a cap height.
///
/// WHY the face is measured through the typeface and not through the
/// bitmap's own scale: a cap height is not a scale, and the bitmap face
/// can only be asked about whole cells.
fn measure_cap(body: &[u8], cap_px: u32) -> usize {
    let em = std::design::outline_text::em_for_cap_height(cap_px);
    std::design::typeface::text_advance_px(body, em) as usize
}

/// The number of bytes of `body` that fit in `max_px` at the given cap height.
///
/// WHY this is measured and not counted: the platform's face is
/// proportional, so a string's width depends on the letters in it. Counting
/// characters would let a wide string run past the column it belongs to.
fn fit(body: &[u8], max_px: usize, cap_px: u32) -> usize {
    // WHY the byte limit comes first: a run longer than the face's key is
    // refused by the face and drawn by the bitmap, and the bitmap draws at
    // a whole cell, so the run would come out at a different size from
    // every other run on the screen.
    let limit = body.len().min(MAX_RUN_BYTES);
    if measure_cap(&body[..limit], cap_px) <= max_px {
        return limit;
    }
    let mut len = 0usize;
    while len < limit {
        if measure_cap(&body[..len + 1], cap_px) > max_px {
            break;
        }
        len += 1;
    }
    len
}

/// The number of bytes of `body` that fit in `max_px`, copied into `out` and
/// marked when the run was cut.
///
/// WHY the mark belongs here and not in `fit`: `fit` is a measurement and has
/// nowhere to put the mark, so the mark is written while the bytes are being
/// copied, and the two can never disagree about where the run ends.
fn fit_marked(body: &[u8], max_px: usize, cap_px: u32, out: &mut [u8]) -> usize {
    let len = fit(body, max_px, cap_px);
    // WHY the clamp: a destination too small for the run must not be written
    // past, and the returned length is what the caller slices with.
    let len = len.min(out.len());
    out[..len].copy_from_slice(&body[..len]);
    if len < body.len() && len >= 3 {
        out[len - 3..len].copy_from_slice(b"...");
    }
    len
}

// -----------------------------------------------------------------------------
// Painting
// -----------------------------------------------------------------------------

/// The rows the platform's band owns on this screen.
pub fn status_band() -> (usize, usize) {
    (0, STATUS_BAR_HEIGHT)
}

/// Reads the wall clock once and says whether the band's lane changed.
///
/// WHY it returns the change rather than redrawing: the loop repaints only what
/// actually moved, and a clock that answers the same minute costs no frame.
/// WHY an unanswered RTC leaves the lane blank: the read needs the
/// `RealtimeClockRead` capability, and a placeholder time is a measurement the
/// device never took.
fn refresh_clock(app: &mut AgentManagerApp) -> bool {
    let mut clock = [b' '; CLOCK_LEN];
    let mut clock_ok = false;
    if let Some(now) = std::rtc::Reading::now() {
        clock = now.clock_text();
        clock_ok = true;
    }
    let changed = clock_ok != app.clock_ok || (clock_ok && clock != app.clock);
    app.clock = clock;
    app.clock_ok = clock_ok;
    changed
}

/// The colour of one pixel.
fn get_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    // The platform's band, above the app's own bar.
    //
    // WHY the instruments are unmeasured: this app holds no radio, no fuel
    // gauge and no Wi-Fi association -- what it measures is the model
    // directory and the service registry, and those readings are in its cards,
    // not in the band. A filled bar here would be a level no instrument of this
    // app took.
    if y < STATUS_BAR_HEIGHT {
        let band = components::StatusBand::new(W);
        let clock = if app.clock_ok { Some(&app.clock[..]) } else { None };
        return band
            .pixel(x, y, clock, &components::Instruments::unmeasured(), &THEME)
            .unwrap_or(THEME.surface);
    }
    let base = if y < APPBAR_H {
        appbar_pixel(x, y, app)
    } else if y < BODY_TOP {
        tab_pixel(x, y, app)
    } else {
        let bottom = app.body_bottom();
        if y < bottom {
            body_pixel(x, y, app).unwrap_or(THEME.surface)
        } else if y < ACTION_TOP {
            composer_pixel(x, y, app)
        } else {
            action_pixel(x, y, app)
        }
    };
    // The overflow sheet is on top of everything else, because a sheet that
    // is behind the thing it covers is a panel, not a sheet.
    if let Some(c) = sheet_pixel(x, y, app) {
        return c;
    }
    base
}

/// The app bar: the way out, the surface's name, and the two controls.
///
/// WHY the band is the page's colour and not a card's: the design the
/// showcases describe puts the app bar on the background and lets the
/// cards float on it. A white bar across the top of a grey page is a
/// different design.
fn appbar_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.surface;

    // The back control.
    //
    // WHY the app has one: it is launched from the launcher's drawer, and
    // without a back control the only way out of it is the system gesture.
    // It is drawn in the platform's own place and tested with the
    // platform's own hit region, so the button a person sees and the
    // button a person presses are the same button.
    if let Some(c) = std::design::back_button::pixel(
        x,
        y,
        THEME.on_surface,
        THEME.border,
        THEME.surface,
    ) {
        colour = c;
    }

    // The surface's name. It is the largest thing on the bar, because the
    // name of the screen is the first thing a person looks for.
    let title = app.surface.title();
    if let Some(c) = text(
        x,
        y,
        APPBAR_TITLE_X,
        APPBAR_TITLE_Y,
        CAP_TITLE,
        title.as_bytes(),
        THEME.on_surface,
        THEME.surface,
        true,
    ) {
        colour = c;
    }

    // The search control, which leads to the search surface.
    if let Some(c) = icon_button_px(x, y, appbar_search_rect(), Icon::Search) {
        colour = c;
    }

    // The overflow control, which opens the sheet.
    if let Some(c) = icon_button_px(x, y, appbar_more_rect(), Icon::More) {
        colour = c;
    }

    colour
}

/// The colour of `(x, y)` if it is an app bar icon button.
///
/// WHY the button is a circle on a hairline and not a bare glyph: a glyph
/// on the page is a label, and a label that is also a control is a control
/// nobody presses.
fn icon_button_px(x: usize, y: usize, rect: Rect, icon: Icon) -> Option<u32> {
    let cx = rect.x + rect.w / 2;
    let cy = rect.y + rect.h / 2;
    let mut colour = monochrome::circle_px(
        x,
        y,
        cx,
        cy,
        rect.w / 2,
        THEME.surface_container,
        Some(THEME.border),
    )?;
    let glyph = Rect::new(cx - 11, cy - 11, 22, 22);
    if let Some(c) = monochrome::icon_px(x, y, glyph, icon, THEME.on_surface) {
        colour = c;
    }
    Some(colour)
}

/// The app bar's search control.
fn appbar_search_rect() -> Rect {
    Rect::new(W - GUTTER - ICON_BTN * 2 - ICON_BTN_GAP, ICON_BTN_TOP, ICON_BTN, ICON_BTN)
}

/// The app bar's overflow control.
fn appbar_more_rect() -> Rect {
    Rect::new(W - GUTTER - ICON_BTN, ICON_BTN_TOP, ICON_BTN, ICON_BTN)
}

// ---------------------------------------------------------------------------
// The overflow sheet
// ---------------------------------------------------------------------------

/// The sheet's width.
const SHEET_W: usize = 272;
/// The sheet's row height.
/// A sheet row's height. It follows the type for the same reason.
const SHEET_ROW_H: usize = LINE_BODY + 24;
/// The sheet's padding.
const SHEET_PAD: usize = 8;

/// The rows the sheet holds.
///
/// WHY the sheet holds the actions and not the surfaces: the surfaces are
/// already in the segmented control, and a sheet that repeats them is a
/// sheet with nothing in it.
const SHEET_ROWS: [(Icon, &str, Action); 4] = [
    (Icon::Refresh, "Re-measure device", Action::Refresh),
    (Icon::Activity, "Run self-test", Action::RunTest),
    (Icon::Send, "Open chat", Action::OpenChat),
    (Icon::Close, "Clear conversation", Action::ClearChat),
];

/// The sheet's rectangle.
fn sheet_rect() -> Rect {
    let height = SHEET_PAD * 2 + SHEET_ROWS.len() * SHEET_ROW_H;
    Rect::new(W - GUTTER - SHEET_W, APPBAR_H + 8, SHEET_W, height)
}

/// The colour of `(x, y)` if it is the overflow sheet.
fn sheet_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    if !app.sheet_open {
        return None;
    }
    let rect = sheet_rect();
    if !rect.contains(x, y) {
        return None;
    }
    let mut colour =
        monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border))?;
    for (index, (icon, label, _)) in SHEET_ROWS.iter().enumerate() {
        let row_y = rect.y + SHEET_PAD + index * SHEET_ROW_H;
        let row = Rect::new(rect.x, row_y, rect.w, SHEET_ROW_H);
        if !row.contains(x, y) {
            continue;
        }
        let glyph = Rect::new(rect.x + 16, row_y + (SHEET_ROW_H - 22) / 2, 22, 22);
        if let Some(c) = monochrome::icon_px(x, y, glyph, *icon, THEME.on_surface) {
            colour = c;
        }
        if let Some(c) = text(
            x,
            y,
            rect.x + 50,
            row_y + (SHEET_ROW_H - LINE_BODY) / 2,
            CAP_BODY,
            label.as_bytes(),
            THEME.on_surface,
            THEME.surface_container,
            false,
        ) {
            colour = c;
        }
    }
    Some(colour)
}

// ---------------------------------------------------------------------------
// The floating action button
// ---------------------------------------------------------------------------

/// The floating action button's rectangle.
///
/// WHY it floats over the body instead of sitting in the action bar: a
/// button that is in the bar is a button in a row, and a row of buttons
/// has no primary one. The floating button is the screen's one obvious
/// action, and it is the one a thumb reaches for first.
fn fab_rect(app: &AgentManagerApp) -> Rect {
    // WHY the chat surface has no floating button: the composer carries a
    // send button already, and two controls that do the same thing on one
    // screen is one control too many.
    if app.surface == Surface::Chat {
        return Rect::new(0, 0, 0, 0);
    }
    let bottom = app.body_bottom();
    Rect::new(
        W - GUTTER - FAB_R * 2 - FAB_MARGIN,
        bottom - FAB_R * 2 - FAB_MARGIN,
        FAB_R * 2,
        FAB_R * 2,
    )
}

/// The action the floating button performs on a surface.
fn fab_action(surface: Surface) -> Action {
    match surface {
        Surface::Models => Action::Install,
        Surface::Chat => Action::Send,
        _ => Action::Refresh,
    }
}

/// The glyph the floating button carries on a surface.
fn fab_icon(surface: Surface) -> Icon {
    match surface {
        Surface::Models => Icon::Plus,
        Surface::Chat => Icon::Send,
        _ => Icon::Refresh,
    }
}

/// The colour of `(x, y)` if it is the floating action button.
fn fab_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    let rect = fab_rect(app);
    if rect.w == 0 || rect.h == 0 {
        return None;
    }
    let cx = rect.x + rect.w / 2;
    let cy = rect.y + rect.h / 2;
    let mut colour = monochrome::circle_px(x, y, cx, cy, FAB_R, THEME.primary, None)?;
    let glyph = Rect::new(cx - 13, cy - 13, 26, 26);
    if let Some(c) = monochrome::icon_px(x, y, glyph, fab_icon(app.surface), THEME.on_primary) {
        colour = c;
    }
    Some(colour)
}

/// The segmented control.
fn tab_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.surface;
    // WHY the gap count is derived rather than written down: the tab row
    // has a gap between every pair of segments, so a row of N segments
    // has N-1 gaps. The number was written down, and the last tab in the
    // row ran past the content column the moment a sixth surface was
    // added.
    let segment_w = (CONTENT_W - (Surface::COUNT - 1) * TAB_GAP) / Surface::COUNT;
    let index = (x.saturating_sub(GUTTER)) / (segment_w + TAB_GAP);
    if index < Surface::COUNT {
        let seg_x = GUTTER + index * (segment_w + TAB_GAP);
        let seg = Rect::new(seg_x, TAB_Y, segment_w, TAB_H);
        if seg.contains(x, y) {
            let selected = app.surface.index() == index;
            let (fill, ink) = if selected {
                (THEME.primary, THEME.on_primary)
            } else {
                (THEME.surface_container, THEME.on_surface_variant)
            };
            colour = monochrome::chip_px(
                x,
                y,
                seg,
                fill,
                if selected { None } else { Some(THEME.border) },
            )
            .unwrap_or(fill);

            let label = Surface::ALL[index].tab_label().as_bytes();
            let text_w = measure_cap(label, CAP_CAPTION);
            let text_x = seg_x + segment_w.saturating_sub(text_w) / 2;
            if let Some(c) = text(
                x,
                y,
                text_x,
                TAB_Y + (TAB_H - LINE_CAPTION) / 2,
                CAP_CAPTION,
                label,
                ink,
                fill,
                selected,
            ) {
                colour = c;
            }
        }
    }
    colour
}

/// The scrollable body.
fn body_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    // The floating action button is on top of the body, on top of the last
    // card, because that is where a thumb reaches for it.
    if let Some(c) = fab_pixel(x, y, app) {
        return Some(c);
    }
    body_cards_pixel(x, y, app)
}

/// The scrollable body's cards.
fn body_cards_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    let offset = app.scroll.offset;
    let content_y = y - BODY_TOP + offset;

    for index in 0..app.card_count as usize {
        let card = app.cards[index];
        let card_y = card.y as usize;
        let card_h = card.height as usize;
        if content_y < card_y || content_y >= card_y + card_h {
            continue;
        }

        let top = BODY_TOP as i64 + card_y as i64 - offset as i64;
        let bottom = top + card_h as i64;
        if bottom <= 0 || top >= H as i64 {
            return None;
        }
        let rect_y = top.max(0) as usize;
        let rect_h = (bottom.min(H as i64) - rect_y as i64) as usize;

        if card.kind != CARD_PANEL {
            return bubble_pixel(x, y, app, card, rect_y, rect_h);
        }

        let rect = Rect::new(GUTTER, rect_y, CONTENT_W, rect_h);
        // WHY the card is the platform's monochrome card: a card that invents
        // its own fill and its own hairline is a card a third-party app
        // cannot match, and the design system already says what a card
        // looks like.
        let mut colour =
            monochrome::card_px(x, y, rect, THEME.surface_container, Some(THEME.border))?;

        // The heading.
        let title_top = card_y + CARD_PAD_TOP;
        if content_y < card_y + LINES_TOP {
            let title = &card.title[..card.title_len as usize];
            let mut shown = [0u8; 256];
            let len = fit_marked(title, CONTENT_W - 2 * CARD_PAD_X, CAP_SECTION, &mut shown);
            if let Some(c) = text(
                x,
                y,
                GUTTER + CARD_PAD_X,
                BODY_TOP + title_top - offset,
                CAP_SECTION,
                &shown[..len],
                card.title_tone.colour(),
                THEME.surface_container,
                // WHY the heading is bold: the heading is the name of the
                // thing the rows under it belong to, and a name has to be
                // findable before the rows are read.
                true,
            ) {
                colour = c;
            }
            return Some(colour);
        }

        // The lines.
        let lines_top = card_y + LINES_TOP;
        let mut cursor = lines_top;
        for line_index in 0..card.line_count as usize {
            let line = app.lines[card.line_start as usize + line_index];
            let height = line.rows() * ROW_H;
            if content_y >= cursor && content_y < cursor + height {
                let screen_y = BODY_TOP + cursor - offset;
                if line.wide {
                    let row = (content_y - cursor) / ROW_H;
                    let (start, end) = line.row_range(row);
                    if start < end {
                        let body = &line.value[start..end];
                        let mut shown = [0u8; 256];
                        let len = fit_marked(body, WIDE_W, CAP_CAPTION, &mut shown);
                        // WHY the row's own y and not the line's: a wide
                        // line that wraps has more than one physical row, and
                        // every one of them is a row of its own.
                        if let Some(c) = text(
                            x,
                            y,
                            GUTTER + CARD_PAD_X,
                            screen_y + row * ROW_H,
                            CAP_CAPTION,
                            &shown[..len],
                            line.tone.colour(),
                            THEME.surface_container,
                            false,
                        ) {
                            colour = c;
                        }
                    }
                } else {
                    let label = &line.label[..line.label_len as usize];
                    let mut shown = [0u8; 256];
                    let len = fit_marked(label, LABEL_W - LABEL_GAP, CAP_CAPTION, &mut shown);
                    if let Some(c) = text(
                        x,
                        y,
                        GUTTER + CARD_PAD_X,
                        screen_y + (ROW_H - LINE_CAPTION) / 2,
                        CAP_CAPTION,
                        &shown[..len],
                        THEME.on_surface_variant,
                        THEME.surface_container,
                        false,
                    ) {
                        colour = c;
                    }
                    let value = &line.value[..line.value_len as usize];
                    if line.chip {
                        // WHY a chip and not coloured text: a state written
                        // in colour is a state a person has to read, and a
                        // state written as a dot and a word is a state a
                        // person can find at a glance.
                        if let Some(c) = chip_px(
                            x,
                            y,
                            GUTTER + CONTENT_W - CARD_PAD_X,
                            screen_y + (ROW_H - 26) / 2,
                            value,
                            line.tone,
                        ) {
                            colour = c;
                        }
                    } else {
                        let mut shown = [0u8; 256];
                        // WHY the mark is not written here: a value cut in
                        // the middle of a word reads as a word, and the
                        // reader is left with a different name for the thing
                        // than the platform gave, so the cut and the mark are
                        // decided in one place.
                        let len = fit_marked(value, VALUE_W, CAP_BODY, &mut shown);
                        // WHY the value is right-aligned: a value that starts
                        // at a fixed column leaves a ragged edge, and a list of
                        // ragged values reads as a table of unrelated things.
                        // Aligned to the row's edge, the values form a column
                        // a person can scan down.
                        let value_w = measure_cap(&shown[..len], CAP_BODY);
                        let value_x = (GUTTER + CONTENT_W - CARD_PAD_X).saturating_sub(value_w).max(VALUE_X);
                        if let Some(c) = text(
                            x,
                            y,
                            value_x,
                            screen_y + (ROW_H - LINE_BODY) / 2,
                            CAP_BODY,
                            &shown[..len],
                            line.tone.colour(),
                            THEME.surface_container,
                            false,
                        ) {
                            colour = c;
                        }
                    }
                }
                break;
            }
            cursor += height;
        }
        return Some(colour);
    }

    Some(THEME.surface)
}

/// The colour of `(x, y)` if it is a message bubble.
///
/// WHY the person is on the right and the platform is on the left: the
/// design the showcases describe says so, and a conversation drawn the
/// other way round reads as a log rather than as a conversation.
fn bubble_pixel(
    x: usize,
    y: usize,
    app: &AgentManagerApp,
    card: Card,
    rect_y: usize,
    rect_h: usize,
) -> Option<u32> {
    let width = (card.width as usize).min(CONTENT_W);
    let left = if card.kind == CARD_BUBBLE_ME {
        W - GUTTER - width
    } else {
        GUTTER
    };
    let rect = Rect::new(left, rect_y, width, rect_h);
    let (fill, ink) = if card.kind == CARD_BUBBLE_ME {
        (THEME.primary, THEME.on_primary)
    } else {
        (THEME.surface_container_low, THEME.on_surface)
    };
    let mut colour = monochrome::card_px(x, y, rect, fill, None)?;

    let mut cursor = rect_y + BUBBLE_PAD;
    for line_index in 0..card.line_count as usize {
        let line = app.lines[card.line_start as usize + line_index];
        for row in 0..line.rows() {
            let (start, end) = line.row_range(row);
            let body = &line.value[start..end];
            let mut shown = [0u8; 256];
            let len = fit_marked(
                body,
                width.saturating_sub(2 * BUBBLE_PAD),
                CAP_CAPTION,
                &mut shown,
            );
            if let Some(c) = text(
                x,
                y,
                left + BUBBLE_PAD,
                cursor,
                CAP_CAPTION,
                &shown[..len],
                ink,
                fill,
                false,
            ) {
                colour = c;
            }
            cursor += ROW_H;
        }
    }
    Some(colour)
}

/// The colour of `(x, y)` if it is a status chip whose right edge is at
/// `right_x`.
fn chip_px(
    x: usize,
    y: usize,
    right_x: usize,
    top: usize,
    label: &[u8],
    tone: Tone,
) -> Option<u32> {
    let text_w = measure_cap(label, CAP_CAPTION);
    let width = 10 + 8 + 6 + text_w + 10;
    let left = right_x.saturating_sub(width);
    let rect = Rect::new(left, top, width, 26);
    let mut colour = monochrome::chip_px(x, y, rect, THEME.surface_container_low, None)?;
    let dot_cx = left + 10 + 4;
    let dot_cy = top + 13;
    if let Some(c) = monochrome::circle_px(x, y, dot_cx, dot_cy, 4, tone.colour(), None) {
        colour = c;
    }
    if let Some(c) = text(
        x,
        y,
        dot_cx + 10,
        top + (LINE_CAPTION + 12).saturating_sub(LINE_CAPTION) / 2,
        CAP_CAPTION,
        label,
        tone.colour(),
        THEME.surface_container_low,
        false,
    ) {
        colour = c;
    }
    Some(colour)
}

/// The composer band, on the chat surface.
///
/// WHY the composer is pinned rather than scrolled: a composer that scrolls
/// away is a composer that cannot be reached while reading the answer, and
/// the answer is the reason the person is here.
fn composer_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.surface;
    if app.surface != Surface::Chat {
        return colour;
    }
    let top = ACTION_TOP - COMPOSER_H;
    if y == top {
        colour = THEME.border;
    }
    let field_y = top + COMPOSER_H.saturating_sub(COMPOSER_FIELD_H) / 2;
    let send_cx = W - GUTTER - COMPOSER_SEND_R;
    let send_cy = top + COMPOSER_H / 2;
    let field_right = send_cx.saturating_sub(COMPOSER_SEND_R + 12);
    let field = Rect::new(
        GUTTER,
        field_y,
        field_right.saturating_sub(GUTTER),
        COMPOSER_FIELD_H,
    );
    if field.contains(x, y) {
        if let Some(c) =
            monochrome::control_px(x, y, field, THEME.surface_container, Some(THEME.border))
        {
            colour = c;
        }
        let prompt = app.chat.prompt_text();
        let (body, ink) = if app.chat.prompt_is_empty() {
            (&b"Type a message..."[..], THEME.muted)
        } else {
            (prompt, THEME.on_surface)
        };
        let mut shown = [0u8; 256];
        let len = fit_marked(body, field.w.saturating_sub(28), CAP_BODY, &mut shown);
        if let Some(c) = text(
            x,
            y,
            field.x + 14,
            field_y + (COMPOSER_FIELD_H - LINE_BODY) / 2,
            CAP_BODY,
            &shown[..len],
            ink,
            THEME.surface_container,
            false,
        ) {
            colour = c;
        }
    }
    if let Some(c) = monochrome::circle_px(
        x,
        y,
        send_cx,
        send_cy,
        COMPOSER_SEND_R,
        THEME.primary,
        None,
    ) {
        colour = c;
    }
    if let Some(c) = monochrome::icon_px(
        x,
        y,
        Rect::new(send_cx - 12, send_cy - 12, 24, 24),
        Icon::Send,
        THEME.on_primary,
    ) {
        colour = c;
    }
    colour
}

/// The bottom action bar.
fn action_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.overlay;
    if y == ACTION_TOP {
        colour = THEME.overlay_border;
    }

    let buttons = app.actions();
    let live = buttons.iter().filter(|button| !button.label.is_empty()).count();
    if live > 0 {
        let width = (CONTENT_W - (live - 1) * BUTTON_GAP) / live;
        let mut index = 0usize;
        for button in buttons.iter() {
            if button.label.is_empty() {
                continue;
            }
            let bx = GUTTER + index * (width + BUTTON_GAP);
            let rect = Rect::new(bx, BUTTON_Y, width, BUTTON_H);
            if rect.contains(x, y) {
                let (fill, ink) = if !button.enabled {
                    (THEME.surface_container, THEME.muted)
                } else if index == 0 {
                    (THEME.primary, THEME.on_primary)
                } else {
                    (THEME.surface_container_high, THEME.on_surface)
                };
                // WHY the primary button has no hairline: a light line
                // around a black button is a seam, and a seam on the one
                // control a person is meant to press is a thing that looks
                // like a mistake.
                colour = monochrome::chip_px(
                    x,
                    y,
                    rect,
                    fill,
                    if index == 0 { None } else { Some(THEME.border) },
                )
                .unwrap_or(fill);
                let text_w = measure_cap(button.label.as_bytes(), CAP_CAPTION);
                let text_x = bx + width.saturating_sub(text_w) / 2;
                if let Some(c) = text(
                    x,
                    y,
                    text_x,
                    BUTTON_Y + (BUTTON_H - LINE_CAPTION) / 2,
                    CAP_CAPTION,
                    button.label.as_bytes(),
                    ink,
                    fill,
                    true,
                ) {
                    colour = c;
                }
            }
            index += 1;
        }
    }

    // The last action's result.
    let message = &app.action_log[..app.action_log_len];
    if y >= RESULT_Y && y < RESULT_Y + LINE_CAPTION + 4 {
        let len = fit(message, CONTENT_W, CAP_CAPTION);
        // WHY the mark: a status line that stops in the middle of a word
        // reads as a word, and a reader who takes it for one is reading a
        // sentence the platform never said. The mark says the sentence
        // continues.
        let mut shown = [0u8; ACTION_LOG_MAX];
        shown[..len].copy_from_slice(&message[..len]);
        let len = if len < message.len() && len >= 3 {
            shown[len - 3..len].copy_from_slice(b"...");
            len
        } else {
            len
        };
        if let Some(c) = text(
            x,
            y,
            GUTTER,
            RESULT_Y,
            CAP_CAPTION,
            &shown[..len],
            app.action_tone.colour(),
            THEME.overlay,
            false,
        ) {
            colour = c;
        }
    }

    // The home indicator.
    let gesture = Rect::new((W - GESTURE_W) / 2, GESTURE_Y, GESTURE_W, 5);
    if gesture.contains(x, y) {
        colour = std::design::primitives::draw_pill(x, y, gesture, THEME.overlay_border, None)
            .unwrap_or(THEME.overlay_border);
    }

    colour
}

// -----------------------------------------------------------------------------
// Input
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dirty {
    None,
    All,
}

/// What a tap did.
enum Tap {
    None,
    Redraw,
    Leave(&'static str),
}

/// Which body line a tap landed on, and what it would do.
fn hit_body(app: &AgentManagerApp, x: usize, y: usize) -> Option<i8> {
    if x < GUTTER || x >= GUTTER + CONTENT_W {
        return None;
    }
    let content_y = y - BODY_TOP + app.scroll.offset;
    for index in 0..app.card_count as usize {
        let card = app.cards[index];
        let card_y = card.y as usize;
        let card_h = card.height as usize;
        if content_y < card_y || content_y >= card_y + card_h {
            continue;
        }
        // WHY a bubble is skipped: a bubble is a message, and a message is
        // not something a tap selects.
        if card.kind != CARD_PANEL {
            continue;
        }
        let mut cursor = card_y + LINES_TOP;
        for line_index in 0..card.line_count as usize {
            let line = app.lines[card.line_start as usize + line_index];
            let height = line.rows() * ROW_H;
            if content_y >= cursor && content_y < cursor + height {
                if line.action >= 0 {
                    return Some(line.action);
                }
                return None;
            }
            cursor += height;
        }
        return None;
    }
    None
}

/// Handles a tap.
fn handle_tap(app: &mut AgentManagerApp, x: u32, y: u32) -> Tap {
    let ux = x as usize;
    let uy = y as usize;

    // The back control in the app bar.
    //
    // WHY the rows are the control's own: the region used to reach up into the
    // band and stop short of the control's bottom, so a tap on the lower half
    // of the button the reader was looking at did nothing and a tap on the band
    // did.
    if std::design::back_button::contains(ux, uy)
        || (ux >= 24 && ux < 88 && uy >= HEADER_TOP && uy < HEADER_TOP + std::design::back_button::SIZE)
    {
        return Tap::Leave("[AGENT_MGR] Back button tapped. Returning to the launcher.");
    }

    // The home indicator.
    if (uy >= GESTURE_Y - 20 || uy >= 1395) && (200..=520).contains(&ux) {
        return Tap::Leave("[AGENT_MGR] Home indicator tapped. Returning to the launcher.");
    }

    // The overflow sheet, which is on top of everything.
    //
    // WHY a tap outside the sheet closes it and does nothing else: a sheet
    // that swallows every tap under it is a sheet the reader cannot get
    // past, and a sheet that acted on the tap under it would be a button
    // the person cannot see they are pressing.
    if app.sheet_open {
        let sheet = sheet_rect();
        if !sheet.contains(ux, uy) {
            app.sheet_open = false;
            return Tap::Redraw;
        }
        for (index, (_icon, _label, action)) in SHEET_ROWS.iter().enumerate() {
            let row_y = sheet.y + SHEET_PAD + index * SHEET_ROW_H;
            if uy >= row_y && uy < row_y + SHEET_ROW_H {
                app.sheet_open = false;
                match action {
                    Action::Refresh => app.refresh(),
                    Action::RunTest => app.run_self_test(),
                    Action::OpenChat => {
                        app.surface = Surface::Chat;
                        app.scroll = ScrollState::new(app.content_height as usize, BODY_H, 0);
                        app.rebuild();
                    }
                    Action::ClearChat => app.clear_chat(),
                    _ => {}
                }
                return Tap::Redraw;
            }
        }
        return Tap::None;
    }

    // The app bar's search control.
    if appbar_search_rect().contains(ux, uy) {
        if app.surface != Surface::Search {
            app.surface = Surface::Search;
            app.scroll = ScrollState::new(app.content_height as usize, BODY_H, 0);
            app.rebuild();
        }
        return Tap::Redraw;
    }

    // The app bar's overflow control.
    if appbar_more_rect().contains(ux, uy) {
        app.sheet_open = true;
        return Tap::Redraw;
    }

    // The segmented control.
    if uy >= TAB_Y && uy < TAB_Y + TAB_H {
        // WHY the gap count is derived: see the note at the tab row's
        // own geometry.
        let segment_w = (CONTENT_W - (Surface::COUNT - 1) * TAB_GAP) / Surface::COUNT;
        let index = (ux.saturating_sub(GUTTER)) / (segment_w + TAB_GAP);
        if let Some(surface) = Surface::from_index(index) {
            if surface != app.surface {
                app.surface = surface;
                app.scroll = ScrollState::new(app.content_height as usize, BODY_H, 0);
                app.rebuild();
                return Tap::Redraw;
            }
        }
        return Tap::None;
    }

    // The bottom action bar.
    if uy >= BUTTON_Y && uy < BUTTON_Y + BUTTON_H {
        let buttons = app.actions();
        let live = buttons.iter().filter(|button| !button.label.is_empty()).count();
        if live > 0 {
            let width = (CONTENT_W - (live - 1) * BUTTON_GAP) / live;
            let mut index = 0usize;
            for button in buttons.iter() {
                if button.label.is_empty() {
                    continue;
                }
                let bx = GUTTER + index * (width + BUTTON_GAP);
                if ux >= bx && ux < bx + width {
                    if !button.enabled {
                        return Tap::None;
                    }
                    match button.action {
                        Action::Refresh => app.refresh(),
                        Action::Install => app.install_selected(),
                        Action::Remove => app.remove_selected(),
                        Action::RunTest => app.run_self_test(),
                        Action::RunTools => app.run_tools(),
                        Action::CancelStream => app.cancel_streaming(),
                        Action::Send => app.send_prompt(),
                        Action::ClearChat => app.clear_chat(),
                        Action::OpenChat => {
                            app.surface = Surface::Chat;
                            app.scroll = ScrollState::new(app.content_height as usize, BODY_H, 0);
                            app.rebuild();
                        }
                    }
                    return Tap::Redraw;
                }
                index += 1;
            }
        }
        return Tap::None;
    }

    // The composer's send button.
    if app.surface == Surface::Chat && uy >= ACTION_TOP - COMPOSER_H && uy < ACTION_TOP {
        let send_cx = W - GUTTER - COMPOSER_SEND_R;
        let send_cy = ACTION_TOP - COMPOSER_H + COMPOSER_H / 2;
        let dx = ux as i64 - send_cx as i64;
        let dy = uy as i64 - send_cy as i64;
        if dx * dx + dy * dy <= (COMPOSER_SEND_R * COMPOSER_SEND_R) as i64 {
            if app.chat.is_streaming {
                app.cancel_streaming();
            } else {
                app.send_prompt();
            }
            return Tap::Redraw;
        }
        return Tap::None;
    }

    // The floating action button.
    if app.surface != Surface::Chat && uy >= BODY_TOP && uy < app.body_bottom() {
        let fab = fab_rect(app);
        let cx = fab.x + fab.w / 2;
        let cy = fab.y + fab.h / 2;
        let dx = ux as i64 - cx as i64;
        let dy = uy as i64 - cy as i64;
        if dx * dx + dy * dy <= (FAB_R * FAB_R) as i64 {
            match fab_action(app.surface) {
                Action::Install => app.install_selected(),
                Action::Send => app.send_prompt(),
                _ => app.refresh(),
            }
            return Tap::Redraw;
        }
    }

    // The body.
    if uy >= BODY_TOP && uy < app.body_bottom() {
        if let Some(action) = hit_body(app, ux, uy) {
            app.select_provider(action as usize);
            return Tap::Redraw;
        }
        return Tap::None;
    }

    Tap::None
}

// -----------------------------------------------------------------------------
// The shell
// -----------------------------------------------------------------------------

/// Paints the screen.
fn repaint(surface: u32, app: &AgentManagerApp) {
    std::design::outline_text::reset_slot_pool();
    let mut chunk = [0u32; CHUNK];
    let mut offset = 0usize;
    while offset < W * H {
        let count = (W * H - offset).min(CHUNK);
        let mut x = offset % W;
        let mut y = offset / W;
        for index in 0..count {
            chunk[index] = get_pixel(x, y, app);
            x += 1;
            if x == W {
                x = 0;
                y += 1;
            }
        }
        let _ = std::surface_write(surface, offset, &chunk[..count]);
        offset += count;
    }
}

/// Paints the platform band's rows and nothing else.
///
/// WHY a lane of its own: the clock moves while nobody is touching the screen,
/// and answering a minute with a whole-screen repaint costs the reader a frame
/// for five glyphs' worth of change.
fn repaint_band(surface: u32, app: &AgentManagerApp) {
    let mut row = [0u32; W];
    for y in 0..STATUS_BAR_HEIGHT {
        for (x, slot) in row.iter_mut().enumerate() {
            *slot = get_pixel(x, y, app);
        }
        let _ = std::surface_write(surface, y * W, &row);
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log("[AGENT_MGR (Ring 3)] Agent Manager online.").ok();

    let mut app = AgentManagerApp::new();

    let surface = match std::surface_create(W as u32, H as u32, 200) {
        Ok(surface) => surface,
        Err(_) => {
            // WHY it refuses rather than running headless: a supervisor with
            // no surface has nobody to supervise for, and a silent exit would
            // look like a clean start.
            std::log("[AGENT_MGR] Display surface unavailable; refusing headless execution.").ok();
            std::exit_with_code(1);
        }
    };

    refresh_clock(&mut app);
    repaint(surface, &app);
    let _ = std::surface_flip();

    loop {
        // WHY the pipeline is drained before the wait, and not only after a
        // touch woke the app: a key press produces no touch event, so an
        // app that waits for a touch before it reads the pipeline never
        // sees a key until somebody touches the screen. The arrow keys
        // were dead for that reason.
        let mut input = std::InputEvent {
            kind: 0,
            x: 0,
            y: 0,
            dx: 0,
            dy: 0,
            scale_milli: 1000,
        };
        let mut dirty = Dirty::None;
        let mut handled = false;
        while std::input_event_poll(&mut input).is_ok() {
            handled = true;
            if let Some(key) = input.key_event() {
                if key.pressed {
                    // WHY the keys go to the composer only on the chat
                    // surface: on every other surface a key press is the
                    // platform's, and swallowing it would make the app look
                    // broken.
                    if app.surface == Surface::Chat {
                        match handle_chat_key(&mut app, key.code) {
                            ChatKey::Edited => dirty = Dirty::All,
                            ChatKey::Ignored => {}
                        }
                    } else if handle_scroll_key(&mut app, key.code) {
                        dirty = Dirty::All;
                    }
                }
                continue;
            }
            if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10
            {
                // A drag over the body scrolls it; anything else is the
                // system's navigation gesture.
                //
                // WHY the body keeps the gesture: the body is a scrollable
                // list, and a list that cannot be scrolled is a list whose
                // lower half cannot be reached.
                let over_body =
                    (input.y as usize) >= BODY_TOP && (input.y as usize) < app.body_bottom();
                if (input.kind == 4 || input.kind == 10) && over_body {
                    app.scroll.apply_drag(input.dy as i32);
                    dirty = Dirty::All;
                    continue;
                }
                std::log("[AGENT_MGR] Navigation gesture recognized. Returning to the launcher.").ok();
                let _ = std::surface_destroy(surface);
                std::exit_with_code(0);
            }
            if input.kind == 1 || input.kind == 2 {
                match handle_tap(&mut app, input.x, input.y) {
                    Tap::Leave(message) => {
                        std::log(message).ok();
                        let _ = std::surface_destroy(surface);
                        std::exit_with_code(0);
                    }
                    Tap::Redraw => dirty = Dirty::All,
                    Tap::None => {}
                }
            }
        }
        if app.chat.is_streaming {
            if app.step_streaming() {
                dirty = Dirty::All;
            }
        } else if !handled {
            // Nothing was waiting: sleep until something arrives.
            let _ = std::event_poll(std::EVENT_TOUCH_INPUT, 100);
        }
        // WHY the clock is read here and not in the rasteriser: the band is a
        // few glyphs, the read is a syscall, and the screen is 720x1440 pixels.
        let clock_moved = refresh_clock(&mut app);
        if dirty == Dirty::All {
            repaint(surface, &app);
            let _ = std::surface_flip();
        } else if clock_moved {
            repaint_band(surface, &app);
            let _ = std::surface_flip();
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let mut msg = [0u8; 128];
    let mut n = 0;
    for &b in b"[PANIC] Agent Manager: " {
        msg[n] = b;
        n += 1;
    }
    if let Some(loc) = info.location() {
        let f = loc.file().as_bytes();
        let flen = f.len().min(40);
        msg[n..n + flen].copy_from_slice(&f[..flen]);
        n += flen;
        msg[n] = b':';
        n += 1;
        n += agent_manager::format_u32(loc.line(), &mut msg[n..]);
    }
    if let Ok(s) = core::str::from_utf8(&msg[..n]) {
        std::log(s).ok();
    } else {
        std::log("[PANIC] Agent Manager app error!").ok();
    }
    std::exit_with_code(1);
}
