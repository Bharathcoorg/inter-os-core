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
use std::design::components::{self, Elevation};
use std::design::font;
use std::design::primitives::Rect;
use std::design::scroll::ScrollState;
use agent_manager::theme::THEME;

use agent_manager::{
    format_threshold, format_u32, granted_capability_count, probe_capability_grants, providers,
    resolve_inference_service, service_name, CapabilityGrant, ChatRole, ChatSession, DeviceState,
    InstallState, ModelProbeStatus, ProbeStatus, Provider, RuntimeBinding, SearchConfiguration,
    SelfTest, ServiceState, Surface, CHAT_PROMPT_MAX, CAPABILITY_COUNT, MATRYOSHKA_DEFAULT_DIM,
    MAX_RESULTS_DEFAULT, SIMILARITY_DEFAULT_Q15,
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

/// The status bar's height, from the platform's viewport metrics.
const STATUS_H: usize = 84;

/// The segmented control's band.
const TAB_Y: usize = 92;
const TAB_H: usize = 44;
const TAB_GAP: usize = 5;

/// The scrollable body's band.
const BODY_TOP: usize = 152;
const BODY_BOTTOM: usize = 1284;
const BODY_H: usize = BODY_BOTTOM - BODY_TOP;
/// The bottom action bar's band.
const ACTION_TOP: usize = 1284;
const BUTTON_Y: usize = 1304;
const BUTTON_H: usize = 56;
const BUTTON_GAP: usize = 8;
const RESULT_Y: usize = 1372;

/// The home indicator.
const GESTURE_Y: usize = 1414;
const GESTURE_W: usize = 160;

// WHY the action bar starts where the body ends: the two bands share an
// edge, and a gap between them would be a strip of page that belongs to
// neither. The compiler checks the two numbers agree.
const _: () = assert!(ACTION_TOP == BODY_BOTTOM);

// The body model.
const CARD_PAD_X: usize = 16;
const CARD_PAD_TOP: usize = 12;
const CARD_PAD_BOTTOM: usize = 16;
const CARD_GAP: usize = 12;
const TITLE_ROW_H: usize = 72;
const TITLE_GAP: usize = 8;
const ROW_H: usize = 48;
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
const TYPE_CAPTION: usize = THEME.type_scale.caption.scale;
const TYPE_BODY: usize = THEME.type_scale.body.scale;
/// The scale a heading is drawn at.
///
/// WHY the heading and the body were both at the body scale: the
/// screen had no hierarchy, and a screen whose every level of text is
/// the same size is a screen that reads as a dump. The heading is a
/// step up from the body.
const TYPE_TITLE: usize = THEME.type_scale.title.scale;

/// How many cards the body can hold.
const MAX_CARDS: usize = 12;
/// How many lines the body can hold.
const MAX_LINES: usize = 28;
/// The longest label a line carries.
const LABEL_MAX: usize = 30;
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
/// of an exchange have to be tellable apart at a glance.
fn chat_role_tone(role: ChatRole) -> Tone {
    match role {
        ChatRole::User => Tone::Accent,
        ChatRole::Assistant => Tone::Good,
        ChatRole::Notice => Tone::Unavailable,
    }
}

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
fn wrap_rows(value: &[u8]) -> ([u8; WIDE_ROWS], u8) {
    let mut starts = [0u8; WIDE_ROWS];
    let mut count = 1usize;
    let mut row_start = 0usize;

    while row_start < value.len() && count < WIDE_ROWS {
        let limit = (row_start + WRAP_COLS).min(value.len());
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
    /// Send the prompt to whatever model the device has.
    Send,
    /// Empty the conversation and the prompt.
    ClearChat,
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
}

impl AgentManagerApp {
    /// Builds the app and takes the first measurement.
    pub fn new() -> Self {
        let mut app = Self {
            surface: Surface::Models,
            device: DeviceState::measure(),
            grants: probe_capability_grants(),
            search: SearchConfiguration::measure(),
            self_test: SelfTest::run(),
            selected: 0,
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
        self.grants = probe_capability_grants();
        self.search = SearchConfiguration::measure();
        self.set_action(b"Re-measured: device, grants and search configuration.", Tone::Muted);
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
        let Some(package_id) = provider.package_id() else {
            self.set_action(
                b"INSTALL: this provider is not delivered as a package, so there is nothing to install.",
                Tone::Unavailable,
            );
            self.rebuild();
            return;
        };
        match std::package_install(package_id, true) {
            Ok(()) => {
                self.set_action(b"INSTALL: the platform installed the package.", Tone::Good);
            }
            Err(code) => {
                let mut message = [0u8; ACTION_LOG_MAX];
                let mut n = 0;
                for byte in b"INSTALL refused: " {
                    message[n] = *byte;
                    n += 1;
                }
                n += write_error(&mut message[n..], code);
                self.set_action(&message[..n], Tone::Bad);
            }
        }
        self.device = DeviceState::measure();
        self.rebuild();
    }

    /// Asks the platform to remove the selected provider's package.
    pub fn remove_selected(&mut self) {
        let provider = self.selected_provider();
        let Some(package_id) = provider.package_id() else {
            self.set_action(
                b"REMOVE: this provider is not delivered as a package, so there is nothing to remove.",
                Tone::Unavailable,
            );
            self.rebuild();
            return;
        };
        match std::package_uninstall(package_id, true) {
            Ok(()) => {
                self.set_action(b"REMOVE: the platform removed the package.", Tone::Good);
            }
            Err(code) => {
                let mut message = [0u8; ACTION_LOG_MAX];
                let mut n = 0;
                for byte in b"REMOVE refused: " {
                    message[n] = *byte;
                    n += 1;
                }
                n += write_error(&mut message[n..], code);
                self.set_action(&message[..n], Tone::Bad);
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
        self.cards[index] = card;
        self.card_count += 1;
    }

    /// Appends a line with a label and a value.
    fn add_line(&mut self, label: &[u8], value: &[u8], tone: Tone) {
        self.push_line(label, value, tone, false, -1);
    }

    /// Appends a line that can be tapped to select a provider.
    fn add_selectable(&mut self, label: &[u8], value: &[u8], tone: Tone, action: i8) {
        self.push_line(label, value, tone, false, action);
    }

    /// Appends a line that runs the full width of the card.
    fn add_wide(&mut self, value: &[u8], tone: Tone) {
        self.push_line(b"", value, tone, true, -1);
    }

    fn push_line(&mut self, label: &[u8], value: &[u8], tone: Tone, wide: bool, action: i8) {
        let index = self.line_count as usize;
        if index >= MAX_LINES {
            return;
        }
        let mut line = Line::empty();
        let label_len = label.len().min(LABEL_MAX);
        line.label[..label_len].copy_from_slice(&label[..label_len]);
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
            let (starts, count) = wrap_rows(&line.value[..line.value_len as usize]);
            line.row_starts = starts;
            line.row_count = count;
        }
        line.action = action;
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
            let mut height = LINES_TOP;
            for line_index in 0..card.line_count as usize {
                let line = self.lines[card.line_start as usize + line_index];
                height += line.rows() * ROW_H;
            }
            height += CARD_PAD_BOTTOM;
            self.cards[index].y = y as u16;
            self.cards[index].height = height as u16;
            y += height + CARD_GAP;
        }
        let content = y.saturating_sub(CARD_GAP);
        self.content_height = content as u16;
        self.scroll = ScrollState::new(content, BODY_H, self.scroll.offset);
    }

    // -------------------------------------------------------------------------
    // The surfaces
    // -------------------------------------------------------------------------

    /// Builds the model management surface.
    fn build_models(&mut self) {
        let models = self.device.models;
        self.begin_card(b"Device model state", Tone::Plain);
        self.add_line(b"Model directory", MODEL_DIRECTORY_LABEL, Tone::Plain);
        self.add_line(b"Directory", models.status.as_str().as_bytes(), model_tone(models.status));
        let mut count = [0u8; 8];
        let n = format_u32(models.entries as u32, &mut count);
        self.add_line(b"Entries", &count[..n], Tone::Plain);
        self.add_line(
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
        self.add_line(
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
            self.add_selectable(
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
        self.add_line(b"Install state", state.as_str().as_bytes(), install_tone(state));
        let binding = selected.runtime_binding(&self.device);
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
        self.add_line(
            b"Package",
            selected.package_id().unwrap_or("none").as_bytes(),
            Tone::Plain,
        );

        self.begin_card(b"Last action", Tone::Plain);
        let mut message = [0u8; ACTION_LOG_MAX];
        let len = self.action_log_len.min(ACTION_LOG_MAX);
        message[..len].copy_from_slice(&self.action_log[..len]);
        self.add_wide(&message[..len], self.action_tone);
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
        self.add_line(b"Granted", &summary[..n], Tone::Plain);
        for grant in grants.iter() {
            self.add_line(
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
        let mut count = [0u8; 12];
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
        self.add_line(
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
            self.begin_card(b"Conversation", Tone::Plain);
            self.add_wide(
                b"No messages yet. Type a prompt and press SEND.",
                Tone::Muted,
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
                self.begin_card(role.label().as_bytes(), chat_role_tone(role));
                self.add_wide(&message.text[..len], chat_role_ink(role));
            }
        }

        self.begin_card(b"Prompt", Tone::Plain);
        if self.chat.prompt_is_empty() {
            self.add_wide(b"Nothing typed yet.", Tone::Muted);
        } else {
            // WHY the prompt is copied out: the builder takes the body
            // mutably and the prompt belongs to the app.
            let mut prompt = [0u8; CHAT_PROMPT_MAX];
            let len = self.chat.prompt_text().len().min(CHAT_PROMPT_MAX);
            prompt[..len].copy_from_slice(&self.chat.prompt_text()[..len]);
            self.add_wide(&prompt[..len], Tone::Plain);
        }
        self.add_wide(
            b"Text is entered with the keyboard. SEND asks the platform's model service.",
            Tone::Muted,
        );
    }

    /// Sends the prompt to whatever model the device has.
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
            // WHY the body is rebuilt here and not only at the end: the early
            // return is what an absent model takes, and without the
            // rebuild the transcript kept saying "No messages yet"
            // after the person had typed a prompt and pressed send.
            self.rebuild();
            return;
        }
        // A service that is running and speaks this version is the only
        // path to a real answer, and the platform does not yet carry the
        // call that would deliver one. The app says so rather than
        // inventing a reply.
        self.chat.push(
            ChatRole::Notice,
            b"The inference service is running, but this build has no way to deliver a request to it.",
        );
        self.set_action(
            b"The model service is running; this build cannot deliver a request to it.",
            Tone::Unavailable,
        );
        self.rebuild();
    }

    /// Empties the conversation and the prompt.
    pub fn clear_chat(&mut self) {
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
        self.add_line(
            b"Overall",
            &summary[..n],
            if test.all_passed {
                Tone::Good
            } else {
                Tone::Bad
            },
        );
        for row in test.rows.iter() {
            self.add_line(
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

        self.begin_card(b"Last action", Tone::Plain);
        let mut message = [0u8; ACTION_LOG_MAX];
        let len = self.action_log_len.min(ACTION_LOG_MAX);
        message[..len].copy_from_slice(&self.action_log[..len]);
        self.add_wide(&message[..len], self.action_tone);
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
            Surface::Chat => [
                ActionButton {
                    label: "SEND",
                    action: Action::Send,
                    // WHY the button is live even when there is no model:
                    // pressing it is how the user finds out, and the
                    // answer is the platform's reason rather than a
                    // button that does nothing.
                    enabled: true,
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
            app.send_prompt();
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
    scale: usize,
    body: &[u8],
    ink: u32,
    bg: u32,
    bold: bool,
) -> Option<u32> {
    if body.is_empty() {
        return None;
    }
    // WHY the outline face at every scale and not only the large ones: the
    // panel is 1080x2160 and this app paints 720x1440, so the compositor
    // magnifies every run by three in width and two in height. A bitmap
    // glyph magnified by three is a soft glyph, and most of the text on
    // this screen is a small run. The outline face is a real outline at
    // the panel's own resolution, so it stays sharp at any size.
    //
    // WHY the bitmap face is still here: a run the outline face refuses
    // -- a character the typeface has no outline for -- has to be drawn
    // somehow, and the bitmap face is what it is drawn with.
    let em = std::design::outline_text::em_for_scale(scale);
    let (ascent, _) = std::design::typeface::line_metrics(em);
    let baseline = (y0 + font::BASELINE * scale).max(ascent as usize);
    if let Some(run) = std::design::outline_text::OutlineRun::place_styled(x0, baseline, em, body, bold) {
        let (bx, by, bw, bh) = run.bounds();
        if x >= bx && x < bx + bw && y >= by && y < by + bh {
            return run.pixel(x, y, ink, bg);
        }
        return None;
    }
    font::test_text_aa(x, y, x0, y0, scale, body, ink, bg, bold)
}

/// The number of bytes of `body` that fit in `max_px` at the given scale.
///
/// WHY this is measured and not counted: the platform's face is
/// proportional, so a string's width depends on the letters in it. Counting
/// characters would let a wide string run past the column it belongs to.
fn fit(body: &[u8], max_px: usize, scale: usize) -> usize {
    if font::measure(body, scale) <= max_px {
        return body.len();
    }
    let mut len = 0usize;
    while len < body.len() {
        if font::measure(&body[..len + 1], scale) > max_px {
            break;
        }
        len += 1;
    }
    len
}

// -----------------------------------------------------------------------------
// Painting
// -----------------------------------------------------------------------------

/// The colour of one pixel.
fn get_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    if y < STATUS_H {
        return status_pixel(x, y, app);
    }
    if y < BODY_TOP {
        return tab_pixel(x, y, app);
    }
    if y < ACTION_TOP {
        return body_pixel(x, y, app).unwrap_or(THEME.surface);
    }
    action_pixel(x, y, app)
}

/// The status bar.
fn status_pixel(x: usize, y: usize, app: &AgentManagerApp) -> u32 {
    let mut colour = THEME.overlay;

    // The app's own mark.
    //
    // WHY this is the one piece of colour in the chrome: the theme reserves
    // its accent for a brand mark, and the mark is the app.
    let tile = Rect::new(GUTTER, (STATUS_H - 32) / 2, 32, 32);
    if tile.contains(x, y) {
        colour = std::design::primitives::draw_rounded_rect(
            x,
            y,
            tile,
            THEME.radii.small,
            THEME.accent,
            None,
            0,
        )
        .unwrap_or(THEME.accent);
        let lx = x - tile.x;
        let ly = y - tile.y;
        let d = (lx as i32 - 16).abs();
        if d <= 9 {
            let target = 24 - 2 * d;
            if (ly as i32 - target).abs() <= 2 {
                colour = THEME.surface;
            }
        }
    }

    // The runtime's state, which is the one thing on this screen that is
    // allowed to be a colour.
    //
    // WHY the pill's place is worked out first: the title and the pill
    // share one band, so the title has to know where the pill is before
    // it can be drawn, or the two overlap and the title is cut.
    let bound = app.device.services.inference_runtime_binding().is_bound();
    let label: &[u8] = if bound { b"RUNTIME BOUND" } else { b"RUNTIME UNBOUND" };
    let ink = if bound { THEME.success } else { THEME.warning };
    let pill_w = font::measure(label, TYPE_CAPTION) + 24;
    let pill_x = W - GUTTER - pill_w;
    let pill = Rect::new(pill_x, (STATUS_H - 30) / 2, pill_w, 30);

    // The title, at the largest scale that leaves the pill its room.
    //
    // WHY the scale is chosen rather than fixed: the title and the status
    // share a band, and a title drawn at a fixed scale is a title that
    // either collides with the status or is smaller than it needs to be.
    let title_room = pill_x.saturating_sub(GUTTER + 44 + 12);
    let title_scale = if font::measure(b"AGENT MANAGER", TYPE_TITLE) <= title_room {
        TYPE_TITLE
    } else if font::measure(b"AGENT MANAGER", TYPE_BODY) <= title_room {
        TYPE_BODY
    } else {
        TYPE_CAPTION
    };
    if let Some(c) = text(
        x,
        y,
        GUTTER + 44,
        6,
        title_scale,
        b"AGENT MANAGER",
        THEME.on_surface,
        THEME.overlay,
        true,
    ) {
        colour = c;
    }
    if pill.contains(x, y) {
        colour = std::design::primitives::draw_pill(
            x,
            y,
            pill,
            THEME.surface_container_high,
            Some(THEME.overlay_border),
        )
        .unwrap_or(THEME.surface_container_high);
        if let Some(c) = text(
            x,
            y,
            pill_x + 12,
            pill.y + 3,
            TYPE_CAPTION,
            label,
            ink,
            THEME.surface_container_high,
            true,
        ) {
            colour = c;
        }
    }

    colour
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
            colour = std::design::primitives::draw_rounded_rect(
                x,
                y,
                seg,
                THEME.radii.chip,
                fill,
                if selected { None } else { Some(THEME.border) },
                TYPE_CAPTION,
            )
            .unwrap_or(fill);

            let label = Surface::ALL[index].tab_label().as_bytes();
            let text_w = font::measure(label, 1);
            let text_x = seg_x + segment_w.saturating_sub(text_w) / 2;
            if let Some(c) = text(x, y, text_x, TAB_Y + 12, 1, label, ink, fill, selected) {
                colour = c;
            }
        }
    }
    // The hairline under the control.
    if y == TAB_Y + TAB_H + 1 {
        colour = THEME.outline_variant;
    }
    colour
}

/// The scrollable body.
fn body_pixel(x: usize, y: usize, app: &AgentManagerApp) -> Option<u32> {
    let offset = app.scroll.offset;
    let content_y = y - BODY_TOP + offset;

    for index in 0..app.card_count as usize {
        let card = app.cards[index];
        let card_y = card.y as usize;
        let card_h = card.height as usize;
        if content_y < card_y || content_y >= card_y + card_h {
            continue;
        }

        let style = components::card_style(&THEME, Elevation::Resting);
        let top = BODY_TOP as i64 + card_y as i64 - offset as i64;
        let bottom = top + card_h as i64;
        if bottom <= 0 || top >= H as i64 {
            return None;
        }
        let rect_y = top.max(0) as usize;
        let rect_h = (bottom.min(H as i64) - rect_y as i64) as usize;
        let rect = Rect::new(GUTTER, rect_y, CONTENT_W, rect_h);
        let mut colour = std::design::primitives::draw_rounded_rect(
            x,
            y,
            rect,
            style.radius,
            style.fill,
            style.border,
            TYPE_CAPTION,
        )?;

        // The heading.
        let title_top = card_y + CARD_PAD_TOP;
        if content_y >= title_top && content_y < title_top + TITLE_ROW_H {
            let title = &card.title[..card.title_len as usize];
            let len = fit(title, CONTENT_W - 2 * CARD_PAD_X, TYPE_TITLE);
            if let Some(c) = text(
                x,
                y,
                GUTTER + CARD_PAD_X,
                BODY_TOP + title_top - offset,
                TYPE_TITLE,
                &title[..len],
                card.title_tone.colour(),
                style.fill,
                // WHY the heading is not bold: a bold run at the heading's size is
                // heavy enough to shout, and a screen whose every heading
                // shouts is a screen with no hierarchy. The heading's size
                // already separates it from the body.
                false,
            ) {
                colour = c;
            }
            return Some(colour);
        }

        // The lines.
        let lines_top = card_y + LINES_TOP;
        if content_y < lines_top {
            return Some(colour);
        }
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
                        let len = fit(body, WIDE_W, TYPE_CAPTION);
                        // WHY the row's own y and not the line's: a wide
                        // line that wraps has more than one physical row, and
                        // every one of them is a row of its own.
                        if let Some(c) = text(
                            x,
                            y,
                            GUTTER + CARD_PAD_X,
                            screen_y + row * ROW_H,
                            TYPE_CAPTION,
                            &body[..len],
                            line.tone.colour(),
                            style.fill,
                            false,
                        ) {
                            colour = c;
                        }
                    }
                } else {
                    let label = &line.label[..line.label_len as usize];
                    let len = fit(label, LABEL_W - LABEL_GAP, TYPE_CAPTION);
                    if let Some(c) = text(
                        x,
                        y,
                        GUTTER + CARD_PAD_X,
                        screen_y,
                        TYPE_CAPTION,
                        &label[..len],
                        THEME.on_surface_variant,
                        style.fill,
                        false,
                    ) {
                        colour = c;
                    }
                    let value = &line.value[..line.value_len as usize];
                    let len = fit(value, VALUE_W, TYPE_BODY);
                    // WHY the mark: a value cut in the middle of a word
                    // reads as a word, and the reader is left with a
                    // different name for the thing than the platform gave.
                    let mut shown = [0u8; 256];
                    shown[..len].copy_from_slice(&value[..len]);
                    let len = if len < value.len() && len >= 3 {
                        shown[len - 3..len].copy_from_slice(b"...");
                        len
                    } else {
                        len
                    };
                    if let Some(c) = text(
                        x,
                        y,
                        VALUE_X,
                        screen_y,
                        TYPE_BODY,
                        &shown[..len],
                        line.tone.colour(),
                        style.fill,
                        false,
                    ) {
                        colour = c;
                    }
                }
                break;
            }
            cursor += height;
        }
        return Some(colour);
    }
    None
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
                colour = std::design::primitives::draw_rounded_rect(
                    x,
                    y,
                    rect,
                    THEME.radii.control,
                    fill,
                    Some(THEME.border),
                    TYPE_CAPTION,
                )
                .unwrap_or(fill);
                let text_w = font::measure(button.label.as_bytes(), 1);
                let text_x = bx + width.saturating_sub(text_w) / 2;
                if let Some(c) = text(
                    x,
                    y,
                    text_x,
                    BUTTON_Y + 18,
                    TYPE_CAPTION,
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
    if y >= RESULT_Y && y < RESULT_Y + ROW_H {
        let len = fit(message, CONTENT_W, TYPE_CAPTION);
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
            TYPE_CAPTION,
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

    // The home indicator.
    if uy >= GESTURE_Y - 12 && uy <= GESTURE_Y + 17 && (240..=480).contains(&ux) {
        return Tap::Leave("[AGENT_MGR] Home indicator tapped. Returning to the launcher.");
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
                        Action::Send => app.send_prompt(),
                        Action::ClearChat => app.clear_chat(),
                    }
                    return Tap::Redraw;
                }
                index += 1;
            }
        }
        return Tap::None;
    }

    // The body.
    if uy >= BODY_TOP && uy < BODY_BOTTOM {
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

    repaint(surface, &app);
    let _ = std::surface_flip();

    loop {
        if std::event_poll(std::EVENT_TOUCH_INPUT, 100).is_ok() {
            let mut input = std::InputEvent {
                kind: 0,
                x: 0,
                y: 0,
                dx: 0,
                dy: 0,
                scale_milli: 1000,
            };
            let mut dirty = Dirty::None;
            while std::input_event_poll(&mut input).is_ok() {
                if let Some(key) = input.key_event() {
                    if key.pressed {
                        // WHY the keys go to the composer only on the chat
                        // surface: on every other surface a key press is
                        // the platform's, and swallowing it would make the
                        // app look broken.
                        if app.surface == Surface::Chat {
                            match handle_chat_key(&mut app, key.code) {
                                ChatKey::Edited => dirty = Dirty::All,
                                ChatKey::Ignored => {}
                            }
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
                    let over_body = (input.y as usize) >= BODY_TOP && (input.y as usize) < BODY_BOTTOM;
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
            if dirty == Dirty::All {
                repaint(surface, &app);
                let _ = std::surface_flip();
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log("[PANIC] Agent Manager app error!").ok();
    std::exit_with_code(1);
}
