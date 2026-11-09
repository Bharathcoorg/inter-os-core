// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The boxes the browser paints its chrome controls in, and answers taps in.
//!
//! WHY this is a module and not a number at each call site: the renderer walks
//! the panel one pixel at a time and the press path answers one press at a
//! time, and for most of this app's life each carried its own copy of the
//! rectangle. Every copy was a way for the two to part, and the parting is the
//! defect the owner sees -- a mark on the screen that answers nothing, and a
//! stretch of blank panel that opens something. Each box here is read by the
//! painter and by the press path, so neither can claim an edge the other has
//! not drawn, and the gates under each box are the platform's [`MIN_TARGET`]
//! floor applied to the very constants the painter uses.
//!
//! WHY the boxes carry the sheet's and the field's own origins (the header
//! band, the filter row, the accessory bar): a control whose plate is placed by
//! one rule and whose band is placed by another is the same defect one level
//! down. The origins live beside the boxes they contain, so moving a row moves
//! its control.

use crate::theme::{radius, spacing, type_scale, THEME};
use interos_std::design::font;
use interos_std::design::primitives::Rect;
use interos_std::design::touch::MIN_TARGET;
use interos_std::design::viewport::LOGICAL_WIDTH;

/// The panel width every box is measured against.
///
/// WHY restated: the renderer owns its own `W`, and a control placed from a
/// width the two files disagree about lands off the edge of the screen. The
/// gate in `main.rs` binds this to the renderer's constant at compile time.
pub const PANEL_W: usize = LOGICAL_WIDTH;

/// The gutter every sheet card and list row keeps from the panel's edge.
pub const SIDE_GUTTER: usize = 24;
/// The card's own width between the two gutters.
pub const CARD_W: usize = PANEL_W - 2 * SIDE_GUTTER;

// ---------------------------------------------------------------------------
// The chrome tile: the plate, the ring and the ink every control wears
// ---------------------------------------------------------------------------

/// The two states a chrome control of this app is honestly in.
///
/// WHY only two: the kernel hands this app one tap event (`input.kind == 1`),
/// never a press-down and a release, so there is no moment at which a control
/// *is* pressed for the app to draw. What the app does know is which panel a
/// tile opened: the tile whose sheet is on the screen is the current one, and
/// a control that says "the panel under me is open" is reporting a state it
/// actually holds. A resting/pressed highlight that no event could ever set is
/// a state the app does not have, and would be a lie.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChromeState {
    /// On the bar, doing nothing but waiting.
    Resting,
    /// The tile whose panel the app is currently showing, and the tile whose
    /// job the app is currently doing (Reload while a page is loading is a
    /// Stop, which is a different job on the same plate).
    Current,
}

/// The plate a chrome control is painted on.
///
/// /// The ring that says it is a thing you can press, and the ink its mark
/// takes, are [`tile_ring`] and [`tile_ink`]; the three are one decision about
/// one control, which is why they are three functions over one `ChromeState`
/// rather than colours chosen at each of the nine call sites.
///
/// WHY `surface_container_high` and not the bar's own `surface`: the platform's
/// raised step is the role its own theme names "a raised card, a chip, a
/// pressed control", and it is what [`components::pill`] and
/// [`components::button_style`] (`Secondary`) fill a control with. It is only
/// 1.11:1 off this app's `#FAFAFA` bar, so on its own it is a tone, not an
/// edge -- which is exactly why the edge is [`tile_ring`]'s job and never this
/// step's.
///
/// WHY the current plate is `inverted_surface`: it is the platform's answer to
/// "this one is the one", given by `components::segment_style(_, true)` and
/// `components::pill(.., selected)` for the launcher's tab row and every
/// segmented control on the device, and by `ButtonKind::Primary` for the one
/// control on a screen a person presses. On this theme it is `#0A0A0A` -- the
/// same value as [`Theme::outline`] -- so no colour enters the app.
///
/// [`components::pill`]: interos_std::design::components::pill
/// [`components::button_style`]: interos_std::design::components::button_style
/// [`Theme::outline`]: interos_std::design::theme::Theme::outline
pub const fn tile_plate(state: ChromeState) -> u32 {
    match state {
        ChromeState::Resting => THEME.surface_container_high,
        ChromeState::Current => THEME.inverted_surface(),
    }
}

/// The ring a chrome control is outlined with.
///
/// WHY `outline` and not `border`, and not the app's old `on_surface_variant`:
/// [`Theme::border`] is the `#E5E5E5` hairline that separates two surfaces --
/// 1.21:1 against this bar and 1.08:1 against the plate it would ring, which
/// is a line nobody can see, and is why every control the app rang with it
/// read as a wireframe with no surface in it. The app then moved the ring to
/// `on_surface_variant` (`#525458`, 6.53:1 on the plate), which is *a* line --
/// and is still what the reviewer saw: a hairline outline around a plate one
/// step off the bar, which is a skeleton of a button rather than a button.
///
/// [`Theme::outline`] is the role the theme documents as "a prominent outline:
/// a focused field, a selected control". It is what the SDK's own Back chip
/// rings itself with (`back_button::colors_for_theme` hands out
/// `(on_surface, outline, surface_container_high)`), what File Manager calls
/// `CONTROL_OUTLINE` for every one of its secondary controls, and on this
/// theme it is `#0A0A0A`: 18.97:1 against the bar and 17.15:1 against the
/// plate. WCAG 1.4.11 asks 3:1 of a control's visible boundary; this clears it
/// six times over without leaving the neutral ladder, and it makes the
/// browser's nine chrome tiles and the Back chip beside them one set instead
/// of two.
///
/// WHY the ring does not move on the current tile: `inverted_surface` and
/// `outline` are the same value on this theme, so a tile that fills itself
/// black keeps a continuous edge on a light theme and does not depend on the
/// pairing.
pub const fn tile_ring(state: ChromeState) -> u32 {
    match state {
        ChromeState::Resting | ChromeState::Current => THEME.outline,
    }
}

/// The ink a chrome control's mark is drawn in.
///
/// WHY `on_surface` and not `on_surface_variant`: 17.04:1 on the plate against
/// 6.53:1, and it is the role `back_button::colors_for_theme` gives the
/// chevron it draws on the very same plate. A mark at the muted rung inside a
/// plate whose ring is at the prominent rung reads as the plate's decoration
/// rather than as its label; at `on_surface` the mark is the heaviest thing on
/// the control, which is what a person aims at.
///
/// WHY the current tile takes `on_inverted_surface`: ink follows its surface,
/// and this is the pair `segment_style` and `pill` put on the selected
/// segment.
pub const fn tile_ink(state: ChromeState) -> u32 {
    match state {
        ChromeState::Resting => THEME.on_surface,
        ChromeState::Current => THEME.on_inverted_surface(),
    }
}

/// How wide a ring a chrome control's plate wears.
///
/// WHY one name for it: the painter strokes the ring and the gates below count
/// its pixels, and a width written at each call site is a width that can be
/// edited out on one control while the other nine keep it.
pub const TILE_RING_W: usize = 1;

/// The corner every chrome tile is rounded at.
///
/// WHY a name here and not the `12` the app typed at nine call sites: it is
/// the platform's [`radius::CONTROL`] step -- "buttons, interactive tiles and
/// text fields" -- and the same step the SDK's Back chip rounds at
/// (`back_button::RADIUS`). The gate binds it to both, so the tile cannot stop
/// matching the chip beside it without failing to build.
pub const TILE_R: usize = radius::CONTROL;

const _: () = assert!(TILE_R == radius::CONTROL && TILE_R == 12);
const _: () = assert!(TILE_R == THEME.radii.control);

/// The corner a count badge is cut with: the platform's full-pill step.
///
/// WHY it is a name and not `999` typed at the call site: `draw_rounded_rect`
/// clamps the radius to half the shorter edge, so the step is what turns a
/// square plate into a disc, and a disc whose radius was typed is a disc that
/// quietly becomes a rounded square when someone re-sizes it.
pub const BADGE_R: usize = radius::PILL;

const _: () = assert!(BADGE_R == radius::PILL);

/// The extent every mark on a chrome tile is drawn at.
///
/// WHY the platform's icon box and not each glyph's own hand-tuned size: the
/// browser drew its close mark seventeen rows across, its kebab five, its
/// reload ring nine rows of radius and its plus two rows wide -- every one a
/// different extent, and all of them smaller than the square every other glyph
/// on the device fills. [`interos_std::design::monochrome::ICON_BOX`] is that
/// square, and `monochrome::icon_px` draws the kit's shapes into any box, so a
/// mark here is the kit's mark at the kit's extent at whatever size the plate
/// allows.
pub const ICON: usize = interos_std::design::monochrome::ICON_BOX;

const _: () = assert!(ICON == 24);
const _: () = assert!(ICON < MIN_TARGET);

// ---------------------------------------------------------------------------
// The omnibox row: the address field and the two controls that trail it
// ---------------------------------------------------------------------------
//
// WHY the row is a module and not four rectangles in the renderer: the header
// used to paint the platform's Back chip, the field, reload, a tab count, the
// overflow menu and a close mark, and every one of those six was placed by a
// literal in the file that inked it. Two of them were duplicates of controls the
// screen already had elsewhere -- the navigation bar's own back tile, and the
// shell's home band, which is what this device uses to leave an app -- and a
// third, the tab count, was the same number on the same screen twice. What the
// row actually owns is the address, a way to re-request it, and a way to open
// the menu, so that is what it now paints, and the three boxes are one sum of
// the platform's status band, the platform's control floor and the rung the
// address is set at.

/// The rung the address is inked at.
///
/// WHY published rather than left as the `1` the renderer typed: the field's
/// height, its text row and its caret's box are all one line box of this rung,
/// and a size the ladder does not name is a size the platform's own rung
/// contract rejects.
pub const FIELD_RUNG: usize = crate::theme::type_scale::CAPTION;

/// The platform's status band, which is the row above this one.
pub const OMNIBOX_Y0: usize = interos_std::design::viewport::STATUS_BAR_HEIGHT;

/// The air the band keeps between the status bar and the field's plate, and
/// the same air again under it.
pub const OMNIBOX_DY: usize = spacing::GRID_8;

/// The field's plate height: the platform's control floor plus one grid step.
///
/// WHY not the rung alone: the plate is a thing a thumb is aimed at, so it may
/// not go under [`MIN_TARGET`], and the run inside it is smaller than that floor.
pub const FIELD_H: usize = MIN_TARGET + spacing::GRID_8;

/// The field's top row, and the row one past the band.
pub const FIELD_Y: usize = OMNIBOX_Y0 + OMNIBOX_DY;
pub const OMNIBOX_Y1: usize = FIELD_Y + FIELD_H + OMNIBOX_DY;

/// The band's own last rows: the hairline that reports a navigation in progress.
///
/// WHY it is named here: it is cut out of the band's bottom edge, so a band that
/// grows keeps the hairline on its last rows instead of painting past the row the
/// renderer hands to the page.
pub const PROGRESS_H: usize = 2;
pub const PROGRESS_Y0: usize = OMNIBOX_Y1 - PROGRESS_H;

/// The plate every trailing control is cut to, and the gap two of them keep.
pub const TILE: usize = MIN_TARGET;
pub const TILE_GAP: usize = spacing::GRID_8;

/// How many controls trail the field: reload, then the overflow menu.
///
/// WHY the count is a constant and not the length of a table the painter walks:
/// the field's own width is what is left of the band once the cluster is placed,
/// so adding a control here narrows the address lane by exactly one tile, and the
/// gates below fail the build if the two ever stop agreeing.
pub const OMNIBOX_TRAILING: usize = 2;

/// The plate row of every trailing control: centred on the field, not on the
/// band, so a taller field carries its controls with it.
pub const fn omnibox_tile_y() -> usize {
    FIELD_Y + (FIELD_H - TILE) / 2
}

/// The cluster's left edge: the trailing gutter, less the cluster's own width.
const CLUSTER_X: usize =
    PANEL_W - SIDE_GUTTER - OMNIBOX_TRAILING * TILE - (OMNIBOX_TRAILING - 1) * TILE_GAP;

/// The plate of the `i`th control that trails the field, counted from the left.
pub const fn omnibox_tile(i: usize) -> Rect {
    Rect::new(CLUSTER_X + i * (TILE + TILE_GAP), omnibox_tile_y(), TILE, TILE)
}

/// The address field itself.
pub const OMNIBOX_FIELD: Rect =
    Rect::new(SIDE_GUTTER, FIELD_Y, CLUSTER_X - TILE_GAP - SIDE_GUTTER, FIELD_H);

/// How far inside the field the page's own mark stands, and its square.
///
/// WHY the mark is placed from the field rather than the field from the mark: the
/// picture is the field's decoration, and a plate that moves leaves its mark
/// behind on the column the row above it now owns.
pub const FIELD_MARK_DX: usize = spacing::GRID_16;
pub const FIELD_MARK_SIDE: usize = 16;
pub const FIELD_MARK_X: usize = SIDE_GUTTER + FIELD_MARK_DX;
pub const FIELD_MARK_Y: usize = FIELD_Y + (FIELD_H - FIELD_MARK_SIDE) / 2;

/// How far inside the field the address starts: past the mark, plus the air that
/// separates a label from the glyph it labels.
pub const FIELD_TEXT_DX: usize = FIELD_MARK_DX + FIELD_MARK_SIDE + spacing::GRID_8;
pub const FIELD_TEXT_X: usize = SIDE_GUTTER + FIELD_TEXT_DX;

/// The row the address's line box starts on, centred in the plate by its own
/// height rather than by a remembered offset.
pub const FIELD_TEXT_Y: usize = FIELD_Y + (FIELD_H - font::line_height(FIELD_RUNG)) / 2;

/// The lane the address is cut to: the field, less the inset the run starts at
/// and the same measure again from the trailing edge.
///
/// WHY symmetric: a run cut to the plate's whole remaining width ends its last
/// glyph on the rounding the plate is drawn with, and this painter has no clip.
pub const FIELD_TEXT_LANE: usize = OMNIBOX_FIELD.w - 2 * FIELD_TEXT_DX;

const _: () = assert!(OMNIBOX_Y0 == interos_std::design::viewport::STATUS_BAR_HEIGHT);
const _: () = assert!(FIELD_H >= MIN_TARGET);
const _: () = assert!(OMNIBOX_Y1 > FIELD_Y + FIELD_H);
const _: () = assert!(PROGRESS_Y0 + PROGRESS_H == OMNIBOX_Y1);
const _: () = assert!(OMNIBOX_TRAILING >= 1);
const _: () = assert!(CLUSTER_X > SIDE_GUTTER);
const _: () = assert!(OMNIBOX_FIELD.w >= MIN_TARGET);
const _: () = assert!(OMNIBOX_FIELD.y + OMNIBOX_FIELD.h <= PROGRESS_Y0);
const _: () = assert!(omnibox_tile(0).x == CLUSTER_X);
const _: () = assert!(
    omnibox_tile(OMNIBOX_TRAILING - 1).x + TILE == PANEL_W - SIDE_GUTTER,
    "the trailing cluster must end on the gutter the rest of the app keeps",
);
const _: () = assert!(omnibox_tile(0).y >= OMNIBOX_Y0);
const _: () = assert!(omnibox_tile(0).y + TILE <= PROGRESS_Y0);
const _: () = assert!(omnibox_tile(0).x >= OMNIBOX_FIELD.x + OMNIBOX_FIELD.w + TILE_GAP);
/// The run fits the plate that carries it, with one air above and below.
const _: () = assert!(font::line_height(FIELD_RUNG) <= FIELD_H);
const _: () = assert!(FIELD_TEXT_Y - FIELD_Y == FIELD_Y + FIELD_H - (FIELD_TEXT_Y + font::line_height(FIELD_RUNG)));
const _: () = assert!(FIELD_TEXT_DX > FIELD_MARK_DX + FIELD_MARK_SIDE);
const _: () = assert!(FIELD_TEXT_X + FIELD_TEXT_LANE <= OMNIBOX_FIELD.x + OMNIBOX_FIELD.w);
const _: () = assert!(FIELD_MARK_Y >= FIELD_Y && FIELD_MARK_Y + FIELD_MARK_SIDE <= FIELD_Y + FIELD_H);
/// The mark and the run are one row: the mark's square sits inside the run's own
/// line box, so a page with no picture still shows its mark on the address's line.
const _: () = assert!(FIELD_MARK_SIDE <= font::line_height(FIELD_RUNG));

/// The sheet header's top row, and how tall the header band is.
///
/// WHY both: every sheet draws its title and its close control in this band,
/// and the control has to be inside the strip the sheet actually paints.
pub const SHEET_HEADER_Y0: usize = 120;
pub const SHEET_HEADER_H: usize = 50;

/// Whether `y` is a row of the band a sheet paints its header in.
///
/// WHY a function and not the band's two edges spelled out at each sheet: the
/// ten sheets each inked their title, their close control and the strip behind
/// them under `y >= 120 && y < 170` -- twenty literals beside the two this
/// module already owns at [`SHEET_HEADER_Y0`] and [`SHEET_HEADER_H`]. The repo's
/// rule is that a painted box and the band a press is measured against are the
/// SAME expression, and a literal copied ten times is the way that rule gets
/// broken one sheet at a time: the band edited here would leave ten sheets
/// painting a header the close plate no longer sits inside.
pub const fn in_sheet_header(y: usize) -> bool {
    y >= SHEET_HEADER_Y0 && y < SHEET_HEADER_Y0 + SHEET_HEADER_H
}

/// How far inside the header band a sheet's title is inked, and the row that
/// puts it there.
///
/// WHY derived from the band: the title row was a literal `134` at each of the
/// nine call sites, and a band that moves leaves its ink behind on the row the
/// sheet above it now owns.
pub const SHEET_TITLE_DY: usize = 14;
pub const SHEET_TITLE_Y: usize = SHEET_HEADER_Y0 + SHEET_TITLE_DY;

const _: () = assert!(in_sheet_header(SHEET_HEADER_Y0));
const _: () = assert!(!in_sheet_header(SHEET_HEADER_Y0 + SHEET_HEADER_H));
const _: () = assert!(!in_sheet_header(SHEET_HEADER_Y0 - 1));
const _: () = assert!(
    font::line_height(crate::theme::type_scale::CAPTION) <= SHEET_HEADER_H - SHEET_TITLE_DY
);
const _: () = assert!(in_sheet_header(SHEET_TITLE_Y));

/// The close control a sheet wears in the head of its header band.
///
/// WHY a plate at all: the sheets drew a bare 17-row `X` and answered a press
/// anywhere in a 56x46 strip whose left edge was 12 columns left of the ink and
/// whose top was 17 rows above it, so the person aimed at a glyph they could
/// see and pressed a region of empty header. The plate is the platform's own
/// chrome tile, drawn at the floor a touch target may not go under, and the
/// press answers exactly that plate.
pub const SHEET_CLOSE: Rect = Rect::new(
    PANEL_W - SIDE_GUTTER - MIN_TARGET,
    SHEET_HEADER_Y0 + (SHEET_HEADER_H - MIN_TARGET) / 2,
    MIN_TARGET,
    MIN_TARGET,
);

const _: () = assert!(SHEET_CLOSE.w >= MIN_TARGET && SHEET_CLOSE.h >= MIN_TARGET);
const _: () = assert!(SHEET_CLOSE.y >= SHEET_HEADER_Y0);
const _: () = assert!(SHEET_CLOSE.y + SHEET_CLOSE.h <= SHEET_HEADER_Y0 + SHEET_HEADER_H);
const _: () = assert!(SHEET_CLOSE.x + SHEET_CLOSE.w <= PANEL_W);

/// The top of the filter row in the bookmark and history sheets.
pub const FILTER_Y: usize = 176;

/// The height of that row's card.
///
/// WHY taller than the platform's floor: the clear control has to be the floor,
/// and a 44-row plate set flush against a card that is itself 44 rows tall paints
/// its own corners outside the card's rounding, over the content the sheet is
/// dimming. Eight rows of card on either side of the plate is what keeps the
/// control inside the thing it belongs to.
pub const FILTER_H: usize = 52;

/// The gutter the clear control keeps from the card's trailing edge.
const FILTER_CLEAR_GUTTER: usize = 8;

/// The filter row's card.
pub const FILTER_CARD: Rect = Rect::new(SIDE_GUTTER, FILTER_Y, CARD_W, FILTER_H);

/// The filter's clear control, at the trailing edge of that card.
///
/// WHY it is a plate at all: the row drew a bare 17-row `X` and only when there
/// was something to clear, while the press answered 73 columns of empty card --
/// so half the band people pressed had no control in it, and the ink they aimed
/// at had no edges. Now the plate is drawn where the press answers, and only when
/// it has something to do.
pub const FILTER_CLEAR: Rect = Rect::new(
    PANEL_W - SIDE_GUTTER - FILTER_CLEAR_GUTTER - MIN_TARGET,
    FILTER_Y + (FILTER_H - MIN_TARGET) / 2,
    MIN_TARGET,
    MIN_TARGET,
);

const _: () = assert!(FILTER_CLEAR.w >= MIN_TARGET && FILTER_CLEAR.h >= MIN_TARGET);
const _: () = assert!(FILTER_CLEAR.y > FILTER_Y);
const _: () = assert!(FILTER_CLEAR.y + FILTER_CLEAR.h < FILTER_Y + FILTER_H);
const _: () = assert!(FILTER_CLEAR.x + FILTER_CLEAR.w < FILTER_CARD.x + FILTER_CARD.w);

/// The rows of the sheet lists, and the card each row is drawn in.
///
/// WHY named here: three sheets drew their card `W - 48` wide and then answered
/// a press with `x <= W - 24`, which is the column just past the card's own
/// edge -- the press path claimed a column no pixel of ink was ever painted in.
pub const LIST_ROW_CARD_X: usize = SIDE_GUTTER;
pub const LIST_ROW_CARD_W: usize = CARD_W;

/// The list-row card the press answers, at row top `top` and `h` rows tall.
pub const fn list_row_card(top: usize, h: usize) -> Rect {
    Rect::new(LIST_ROW_CARD_X, top, LIST_ROW_CARD_W, h)
}

/// The width a run inside a list-row card owns.
///
/// WHY a function and not the number each sheet typed: three sheets cut their
/// run to a remembered width (`540`, `580`, `604`) that came from neither the
/// card the painter fills nor the inset the ink sits at. A lane typed from
/// memory is a lane that survives a card re-size by being wrong -- the run
/// either stops short of the card it belongs to or walks out through its
/// border, and the person sees the address lose its tail.
///
/// WHY both an inset and a gutter: the inset is where the ink starts inside the
/// card's leading edge (its icon plate and its own air), and the gutter is what
/// the run keeps from the trailing edge. A run cut to the card's whole remaining
/// width can reach the border exactly, and a per-pixel painter has no clip: the
/// last glyph then sits on the rounding the card itself is drawn with.
pub const fn list_text_lane(inset: usize, gutter: usize) -> usize {
    LIST_ROW_CARD_W.saturating_sub(inset).saturating_sub(gutter)
}

const _: () = assert!(LIST_ROW_CARD_W == PANEL_W - 2 * SIDE_GUTTER);
const _: () = assert!(list_text_lane(0, 0) == LIST_ROW_CARD_W);
const _: () =
    assert!(list_text_lane(SIDE_GUTTER, SIDE_GUTTER) + 2 * SIDE_GUTTER == LIST_ROW_CARD_W);
/// WHY a lane may never be the whole card: a run the width of its card has no
/// edge left in it, and the gate below is what keeps a sheet from re-typing the
/// card's width as a text width by accident.
const _: () = assert!(list_text_lane(46, SIDE_GUTTER) < LIST_ROW_CARD_W);

/// The rung a suggestion card's two runs are set at, and one run's line box.
///
/// WHY published: the row that inks these runs passes the rung to the text
/// painter, so the height the card is built from and the face the ink is set in
/// stay one fact rather than two.
pub const SUGGEST_RUN_SCALE: usize = crate::theme::type_scale::CAPTION;
const SUGGEST_LINE_BOX: usize = font::line_height(SUGGEST_RUN_SCALE);

/// The air a suggestion card keeps above its first line and below its last.
const SUGGEST_PAD_Y: usize = 4;

/// The height of a suggestion card.
///
/// WHY it is derived from the two runs it holds: the card is 68 rows for exactly
/// the reason the history card is -- two line boxes of the rung it sets its text
/// at, plus the air above and below the block. Stating it as the sum is what
/// makes the rows below it a derivation instead of two remembered numbers.
pub const SUGGEST_CARD_H: usize = 2 * SUGGEST_LINE_BOX + 2 * SUGGEST_PAD_Y;

/// The gap between the two suggestion cards.
pub const SUGGEST_ROW_GAP: usize = 10;

pub const SUGGEST_SEARCH: Rect = Rect::new(SIDE_GUTTER, 130, CARD_W, SUGGEST_CARD_H);
/// The second card, stacked off the first: its own last row, plus the gap.
pub const SUGGEST_NAVIGATE: Rect = Rect::new(
    SIDE_GUTTER,
    SUGGEST_SEARCH.y + SUGGEST_CARD_H + SUGGEST_ROW_GAP,
    CARD_W,
    SUGGEST_CARD_H,
);

const _: () = assert!(SUGGEST_SEARCH.h >= MIN_TARGET && SUGGEST_SEARCH.w >= MIN_TARGET);
const _: () = assert!(SUGGEST_SEARCH.y + SUGGEST_SEARCH.h <= SUGGEST_NAVIGATE.y);
const _: () = assert!(SUGGEST_NAVIGATE.y + SUGGEST_NAVIGATE.h <= ACC_BAR_Y0);

/// How far inside a suggestion card its two runs start.
///
/// WHY it is named here: the candidate's width is cut from this card before it
/// reaches the painter, so the inset that decides the cut and the inset the ink
/// is drawn at are two readings of one fact, in two files. A literal at each is
/// a candidate that runs under the icon plate it was cut to clear.
///
/// WHY it is the plate plus its air: the run is set beside the card's mark, and
/// the column it starts at is the mark's own trailing edge plus the gutter that
/// separates a label from the glyph it labels.
pub const SUGGEST_TEXT_AIR: usize = 14;
pub const SUGGEST_TEXT_DX: usize = SUGGEST_ICON_DX + SUGGEST_ICON + SUGGEST_TEXT_AIR;

/// The gutter a suggestion run keeps from the card's trailing edge.
///
/// WHY it exists at all: the run was cut to `SUGGESTION_TEXT_MAX = 604`, which
/// is this card's width less the inset alone -- so a candidate long enough to
/// fill the lane ended on the card's own border, and the last glyph sat on the
/// rounding the card is drawn with. The gutter is the same [`SIDE_GUTTER`] the
/// sheet keeps from the panel, so a row's ink keeps one edge measure everywhere.
pub const SUGGEST_TEXT_GUTTER: usize = SIDE_GUTTER;

/// The lane a suggestion card's run is cut to.
pub const SUGGEST_TEXT_LANE: usize = SUGGEST_SEARCH.w - SUGGEST_TEXT_DX - SUGGEST_TEXT_GUTTER;

/// The square the card's mark is drawn in, and how far inside the card it sits.
///
/// WHY the two offsets are separate: the plate's column is the card's own inset,
/// while its row is what the block leaves over once the square is centred on it.
/// They are the same number today, and the gate below is what says so -- a card
/// that grows its air keeps its plate centred, and a plate whose row was read
/// from its column would climb off the block it labels.
pub const SUGGEST_ICON: usize = 40;
pub const SUGGEST_ICON_DX: usize = 14;
pub const SUGGEST_ICON_DY: usize = (SUGGEST_CARD_H - SUGGEST_ICON) / 2;

/// The centre the plate is drawn round, as an offset from the card's own corner.
pub const SUGGEST_ICON_CX: usize = SUGGEST_ICON_DX + SUGGEST_ICON / 2;
pub const SUGGEST_ICON_CY: usize = SUGGEST_ICON_DY + SUGGEST_ICON / 2;

/// The two rows a suggestion card inks, both measured from its own top.
///
/// WHY they are one block apart: the card used to ink its label at row 14 and
/// its candidate at row 38, which are twenty-four rows apart while the rung both
/// runs are set at reserves thirty. A per-pixel painter has no clip, so the two
/// runs' line boxes overlapped by six rows -- and the candidate's box, at
/// `38 + 30`, reached row 68 of a 68-row card, whose last painted row is 67: the
/// descender of a `g` or a `p` in the address the card offers fell outside the
/// card that offers it.
pub const SUGGEST_LABEL_DY: usize = SUGGEST_PAD_Y;
pub const SUGGEST_TEXT_DY: usize = SUGGEST_PAD_Y + SUGGEST_LINE_BOX;

const _: () = assert!(SUGGEST_TEXT_DX > 0 && SUGGEST_TEXT_GUTTER > 0);
const _: () = assert!(SUGGEST_SEARCH.x + SUGGEST_TEXT_DX + SUGGEST_TEXT_LANE + SUGGEST_TEXT_GUTTER
    == SUGGEST_SEARCH.x + SUGGEST_SEARCH.w);
const _: () = assert!(SUGGEST_TEXT_LANE == list_text_lane(SUGGEST_TEXT_DX, SUGGEST_TEXT_GUTTER));
const _: () = assert!(SUGGEST_LABEL_DY < SUGGEST_TEXT_DY);
const _: () = assert!(SUGGEST_LABEL_DY + SUGGEST_LINE_BOX == SUGGEST_TEXT_DY);
const _: () = assert!(SUGGEST_TEXT_DY + SUGGEST_LINE_BOX + SUGGEST_PAD_Y == SUGGEST_CARD_H);
/// The plate is the card's own square, centred on the block the two runs make.
const _: () = assert!(SUGGEST_ICON_DY == SUGGEST_ICON_DX);
/// WHY the air equals the plate's own inset: the row's ink is set one measure
/// off the mark, the same measure the mark is set off the card's leading edge,
/// so the plate, the air and the card are one rhythm rather than three numbers.
const _: () = assert!(SUGGEST_TEXT_AIR == SUGGEST_ICON_DX);
const _: () = assert!(SUGGEST_TEXT_DX == SUGGEST_ICON_DX + SUGGEST_ICON + SUGGEST_TEXT_AIR);

/// The height of the action button the error, download and dismissal screens
/// draw under their status badge.
///
/// WHY 48 and not the 36 the badge is: the badge is a label, the action is a
/// control, and a control may not be sized under [`MIN_TARGET`].
pub const ACTION_H: usize = 48;

/// The action button's width, shared by the four screens that wear one.
pub const ACTION_W: usize = 180;

/// How far below the status badge a screen's action button stands.
pub const ACTION_DY: usize = 54;

/// The reload action of a failed navigation.
///
/// WHY derived from the badge's row: the badge sits below wrapped body text, so
/// its row moves with the length of the message. The press path used two fixed
/// bands, `630..=720` and `920..=968`, which matched the painted button only for
/// the one body length that happens to land there: a short message put the
/// button's top rows outside the band, a long one drew the button entirely below
/// it -- Reload Page visible and completely dead -- and `920..=968` answered a
/// reload across 90 rows of empty panel where nothing is drawn at all.
pub const fn retry_rect(err_y: usize) -> Rect {
    Rect::new(PANEL_W / 2 - ACTION_W / 2, err_y + ACTION_DY, ACTION_W, ACTION_H)
}

const _: () = assert!(ACTION_W >= MIN_TARGET && ACTION_H >= MIN_TARGET);
const _: () = assert!(retry_rect(SHEET_HEADER_Y0).x + ACTION_W <= PANEL_W);

/// The row of tiles the start page offers under "Quick links".
///
/// WHY the block's top is named too: the heading is drawn above the plates, and
/// the press path must not answer a tile in the heading's rows.
///
/// WHY the whole block is now spaced off the type ladder rather than off four
/// remembered numbers: the heading used to be set at the caption rung inside a
/// band the block cut at `HOME_GRID_Y0 + font::line_height(CAPTION)`, which ran
/// four rows *past* `HOME_TILE_Y` -- so the block painted the top four rows of
/// every tile as flat page, shearing the rounded corners off all four plates --
/// and the captions ran four rows past `HOME_GRID_Y1`, which shaved their
/// descenders and cut the last four rows out of the band that answers them. Both
/// clips were invisible in a screenshot and obvious on the device. Every step of
/// this block is now `line_height(rung)` plus a multiple of the platform's
/// 4-column grid, and the gates below fail the build if a box ever overlaps its
/// neighbour again.
pub const HOME_GRID_Y0: usize = 248;

/// The rung a start-page section heading is set at.
///
/// WHY `BODY` and not `SECTION`: `SECTION` (cap 27) is the rung the platform
/// gives a *screen's* title -- File Manager's `HEADER_TITLE_SCALE`. The heading
/// above a group on a home screen is set at `BODY` in ink `on_surface`, which is
/// how File Manager draws "Recently modified" and "Browse directories", and it
/// is the rung that separates a group from its rows without shouting over the
/// tiles under it.
pub const HOME_HEADING_RUNG: usize = type_scale::BODY;

/// The row a start-page heading's box starts on.
pub const HOME_HEADING_Y: usize = HOME_GRID_Y0 + spacing::GRID_4;
/// How tall a start-page heading's box is.
pub const HOME_HEADING_H: usize = font::line_height(HOME_HEADING_RUNG);

/// The plate row of the tile grid, and how tall the grid block is.
pub const HOME_TILE_Y: usize = HOME_HEADING_Y + HOME_HEADING_H + spacing::GRID_8;

/// The plate's square and the caption it is read with.
pub const HOME_TILE_SIZE: usize = 64;
/// The rung a tile's caption is set at.
///
/// WHY `CAPTION`: the platform's own icon tile draws its label one rung below
/// the plate's title rung -- File Manager's `GRID_NAME_SCALE` is `CAPTION` for
/// exactly this shape -- and the monogram inside the plate already carries the
/// `BODY` rung, so plate and label read as one object with the plate leading.
pub const HOME_CAPTION_RUNG: usize = type_scale::CAPTION;
pub const HOME_TILE_CAPTION_Y: usize = HOME_TILE_Y + HOME_TILE_SIZE + spacing::GRID_4;
/// The height of a tile's caption box.
pub const HOME_CAPTION_H: usize = font::line_height(HOME_CAPTION_RUNG);

/// The first row past the grid block, and so the bottom of a tile's band.
///
/// WHY the band reaches past the plate: the caption is part of the tile -- it is
/// the only thing that says what the tile opens -- and a press on the word
/// "Wikipedia" used to do nothing at all while a press on the icon did.
pub const HOME_GRID_Y1: usize = HOME_TILE_CAPTION_Y + HOME_CAPTION_H + spacing::GRID_4;

const _: () = assert!(HOME_GRID_Y0 < HOME_HEADING_Y);
const _: () = assert!(HOME_HEADING_Y + HOME_HEADING_H < HOME_TILE_Y);
const _: () = assert!(HOME_TILE_Y + HOME_TILE_SIZE <= HOME_TILE_CAPTION_Y);
const _: () = assert!(HOME_TILE_CAPTION_Y + HOME_CAPTION_H <= HOME_GRID_Y1);
const _: () = assert!(HOME_GRID_Y1 > HOME_TILE_Y);

/// The heading row of the start page's second block, "Your pages", and the row
/// the card block under it starts on.
///
/// WHY the pair is derived here: the heading used to be painted inside a band
/// the app wrote as the literals `386..418` while the block above it ended at
/// 376 and the cards below started at 422 -- three numbers that only ever
/// matched by luck, and a heading box that grows is a heading that climbs into
/// the tiles above it or sits on top of the cards below it. Both rows now come
/// off `HOME_GRID_Y1`, the quick-link block's own last row, and the renderer
/// stacks its cards from `HOME_LIST_Y1`, so the two cannot part.
///
/// WHY the cards themselves are NOT derived here: an earlier pass of this sweep
/// did declare their geometry, and nothing painted it -- the renderer and the
/// press path still stack their own four cards, which agree with each other, so
/// there is no duplicated-rect defect to close. Moving that block onto boxes
/// here is a separate change with its own review, not a constant nobody reads.
/// What this module does own is the row the first card stands on, which
/// `main.rs` takes as its stack's anchor, so a heading that grows off its band
/// can no longer paint itself over the cards under it.
pub const HOME_LIST_Y0: usize = HOME_GRID_Y1 + spacing::GRID_8;
/// The row the first card stands on: one heading box and one grid step below
/// the heading's own top.
pub const HOME_LIST_Y1: usize = HOME_LIST_Y0 + HOME_HEADING_H + spacing::GRID_4;

const _: () = assert!(HOME_LIST_Y1 > HOME_LIST_Y0);

/// The columns a tile answers: the whole slice of the strip between its two
/// neighbours, plate and caption together.
///
/// WHY the pitch decides it and not a slop typed under the plate: the caption is
/// part of the control -- it is the only thing that says what the tile opens --
/// and it runs wider than the plate it names. Measured against the typeface,
/// "YouTube" ends nine columns past a band drawn eight columns wider than its
/// plate, so those nine columns of a word the person is aiming at answered
/// nothing. Taking the half-gap each side of the plate gives every tile the
/// columns up to its neighbour's edge, which is what a phone's icon row actually
/// is: no dead strip between two icons, and never one column answered twice.
const TILE_SLOP: usize = (HOME_TILE_CELL - HOME_TILE_SIZE) / 2;

/// How many quick-link tiles the start page offers, and how many it answers.
pub const HOME_TILE_COLUMNS: usize = 4;

/// The slice of the strip one tile owns: the content span, four ways.
///
/// WHY the row is derived from the span instead of typed as four columns: the
/// four plates used to sit at 66, 234, 402 and 570, which is a block spanning
/// 66..634 on a panel whose content runs 24..696 -- ten columns left of the
/// panel's own centre line, with a right margin thirty-eight columns wider than
/// its left. Four equal cells of a quarter of the content span put every plate on
/// its cell's centre and make the four answer-bands tile the content span
/// exactly, with no dead strip at either end.
pub const HOME_TILE_CELL: usize = CARD_W / HOME_TILE_COLUMNS;

const _: () = assert!(CARD_W % HOME_TILE_COLUMNS == 0);
const _: () = assert!(HOME_TILE_CELL > HOME_TILE_SIZE);

/// The plate column of each of the four tiles.
pub const HOME_TILE_X: [usize; 4] = [
    SIDE_GUTTER + HOME_TILE_CELL * 0 + TILE_SLOP,
    SIDE_GUTTER + HOME_TILE_CELL * 1 + TILE_SLOP,
    SIDE_GUTTER + HOME_TILE_CELL * 2 + TILE_SLOP,
    SIDE_GUTTER + HOME_TILE_CELL * 3 + TILE_SLOP,
];

/// The two-letter monogram each tile wears.
///
/// WHY a monogram at all, and why this one: three of the four tiles used to
/// carry one or two letters at the *caption* rung -- sixteen rows of ink in a
/// sixty-four row plate, the same size as the word under them -- and the fourth
/// carried a hand-drawn play triangle. A mark the size of its own label reads as
/// a placeholder that has not been filled in, which is what the reviewer put a
/// finger on. All four now carry the site's initials at the body rung, centred
/// in the plate by measurement.
///
/// WHY `YT` and `WK` beside `IO` and `GH`: one letter among two-letter marks
/// reads as a truncation, and "W" for Wikipedia was the case in point.
///
/// WHY this is not a favicon: the app holds no icon for a start-page link --
/// its `favicon` field is the *loaded* document's picture and nothing else, and
/// the four links here are shipped addresses, not pages this device has visited
/// -- so the honest mark is the initial of the name the caption already prints,
/// not a picture this browser does not have.
pub const HOME_TILE_MONOGRAM: [&[u8]; 4] = [b"IO", b"YT", b"GH", b"WK"];

/// The rung a tile's monogram is set at, and the row its box starts on.
///
/// WHY the body rung: it is the platform's icon-title rung -- `SECTION_CAP_PX`
/// and above it belongs to a screen's title -- and at cap 21 a two-letter mark
/// fills a 64-row plate the way `monochrome::ICON_BOX` fills a chrome tile,
/// instead of floating in it.
pub const HOME_MONOGRAM_RUNG: usize = type_scale::BODY;
pub const HOME_TILE_MONOGRAM_Y: usize =
    HOME_TILE_Y + (HOME_TILE_SIZE - font::line_height(HOME_MONOGRAM_RUNG)) / 2;
/// How far inside the plate a monogram must stay, so the plate reads as a plate
/// and not as a letter with a border.
const HOME_MONOGRAM_INSET: usize = spacing::GRID_8;

const _: () = assert!(font::line_height(HOME_MONOGRAM_RUNG) <= HOME_TILE_SIZE);
const _: () = assert!(HOME_TILE_MONOGRAM_Y + font::line_height(HOME_MONOGRAM_RUNG)
    <= HOME_TILE_Y + HOME_TILE_SIZE - HOME_MONOGRAM_INSET / 2);

/// The column each tile's caption starts at, and the caption itself.
///
/// WHY the caption and its column sit beside the plate's column: the caption is
/// the only thing that says what a tile opens, so it is part of the control, not
/// decoration next to it. The band under `home_tile_hit` answers it, and the gate
/// after the table proves the ink is inside the band that answers it.
///
/// WHY each column is its plate's centre line less half the run's measured
/// width: the caption is set centred under the plate it names, and the band
/// reaches the same `TILE_SLOP` columns on either side of every plate, so the ink
/// it starts outside still lands on this tile's band -- never its neighbour's,
/// which the non-overlap gate holds.
pub const HOME_TILE_CAPTION_X: [usize; 4] = [80, 248, 420, 576];
pub const HOME_TILE_CAPTION: [&[u8]; 4] = [b"InterOS", b"YouTube", b"GitHub", b"Wikipedia"];

/// The address each tile opens, in the same order as its caption.
///
/// WHY the pair is one table: a caption that names a different page than the one
/// its press opens is a control that lies, and the two orders used to live in
/// different files -- the painter's text here, the press path's address there.
pub const HOME_TILE_URL: [&[u8]; 4] = [
    b"https://interos.tech",
    b"https://m.youtube.com",
    b"https://github.com",
    b"https://wikipedia.org",
];

/// The band that answers tile `i`: its plate and its caption.
pub const fn home_tile_hit(i: usize) -> Rect {
    Rect::new(
        HOME_TILE_X[i] - TILE_SLOP,
        HOME_TILE_Y,
        HOME_TILE_SIZE + 2 * TILE_SLOP,
        HOME_GRID_Y1 - HOME_TILE_Y,
    )
}

const _: () = assert!(home_tile_hit(0).w >= MIN_TARGET && home_tile_hit(0).h >= MIN_TARGET);
const _: () = assert!(HOME_GRID_Y0 < HOME_TILE_Y);
const _: () = assert!(HOME_TILE_Y >= SHEET_HEADER_Y0);
const _: () = assert!(HOME_TILE_CAPTION_Y >= HOME_TILE_Y + HOME_TILE_SIZE);
const _: () = assert!(HOME_TILE_CAPTION_Y < HOME_GRID_Y1);
const _: () = assert!(HOME_GRID_Y1 - HOME_TILE_Y >= HOME_TILE_SIZE);
const _: () = assert!(home_tile_hit(3).x + home_tile_hit(3).w <= PANEL_W);
const _: () = assert!(
    home_tile_hit(0).x + home_tile_hit(0).w <= home_tile_hit(1).x
        && home_tile_hit(1).x + home_tile_hit(1).w <= home_tile_hit(2).x
        && home_tile_hit(2).x + home_tile_hit(2).w <= home_tile_hit(3).x,
    "two tiles must never answer the same column",
);
const _: () = assert!(
    {
        let mut i = 0;
        let mut ok = true;
        while i < 4 {
            let hit = home_tile_hit(i);
            if HOME_TILE_CAPTION_X[i] < hit.x {
                ok = false;
            }
            if HOME_TILE_CAPTION_X[i] > HOME_TILE_X[i] + HOME_TILE_SIZE {
                ok = false;
            }
            i += 1;
        }
        ok
    },
    "a tile's caption must start inside the band that answers it, and inside the
     plate's own columns or the slop beside them",
);

/// The top of the accessory strip the keyboard dock carries.
pub const ACC_BAR_Y0: usize = 918;

/// The height of that strip's own plate.
pub const ACC_BAR_H: usize = 56;

/// How far inside the strip a chip stands, and so the chip's top row.
const ACC_CHIP_DY: usize = 6;

/// The height of every chip in the strip.
pub const ACC_CHIP_H: usize = MIN_TARGET;

/// The chip's top row, read by the strip's painter and its press path alike.
pub const ACC_CHIP_Y: usize = ACC_BAR_Y0 + ACC_CHIP_DY;

/// The bottom row of the strip's plate, exclusive.
pub const ACC_BAR_Y1: usize = ACC_BAR_Y0 + ACC_BAR_H;

/// The address chip: the one thing in the strip that is not a name-completer.
pub const ACC_HTTPS: Rect = Rect::new(24, ACC_CHIP_Y, 110, ACC_CHIP_H);
pub const ACC_TLD_COM: Rect = Rect::new(144, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const ACC_TLD_ORG: Rect = Rect::new(234, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const ACC_CLEAR: Rect = Rect::new(324, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const ACC_DISMISS: Rect = Rect::new(630, ACC_CHIP_Y, 64, ACC_CHIP_H);

/// The two chips the strip wears while a page's own field is focused.
pub const FIELD_CLEAR: Rect = Rect::new(24, ACC_CHIP_Y, 80, ACC_CHIP_H);
pub const FIELD_DONE: Rect = Rect::new(600, ACC_CHIP_Y, 94, ACC_CHIP_H);

const _: () = assert!(ACC_HTTPS.w >= MIN_TARGET && ACC_HTTPS.h >= MIN_TARGET);
const _: () = assert!(ACC_TLD_COM.w >= MIN_TARGET && ACC_TLD_ORG.w >= MIN_TARGET);
const _: () = assert!(ACC_CLEAR.w >= MIN_TARGET && ACC_DISMISS.w >= MIN_TARGET);
const _: () = assert!(FIELD_CLEAR.w >= MIN_TARGET && FIELD_DONE.w >= MIN_TARGET);
const _: () = assert!(ACC_CHIP_Y + ACC_CHIP_H <= ACC_BAR_Y1);
const _: () = assert!(ACC_HTTPS.x >= SIDE_GUTTER);
const _: () = assert!(ACC_DISMISS.x + ACC_DISMISS.w <= PANEL_W - SIDE_GUTTER);
const _: () = assert!(FIELD_DONE.x + FIELD_DONE.w <= PANEL_W - SIDE_GUTTER);

#[cfg(test)]
mod tests {
    use super::*;
    use interos_std::design::back_button;
    use interos_std::design::touch::MIN_TARGET;

    /// The omnibox row is two plates and a field, and nothing else.
    ///
    /// WHY the count is pinned in a test: the row used to carry four plates --
    /// reload, the tab count, the overflow menu and a close mark -- beside a
    /// field whose left edge stopped short of the gutter because the platform's
    /// Back chip stood there. Two of those plates duplicated controls the rest of
    /// the screen already had (the bar's tab tile, and the shell's home band), and
    /// the chip duplicated the bar's back tile, so one screen offered two
    /// navigation models and three ways out. The row is now the field plus the two
    /// controls that are only ever the row's own: reload the address, open the
    /// menu.
    #[test]
    fn the_omnibox_row_holds_the_field_and_the_two_controls_that_trail_it() {
        assert_eq!(OMNIBOX_TRAILING, 2);
        // The field takes the panel's own gutter: no column is reserved for a
        // header back affordance, because the row carries none.
        assert_eq!(OMNIBOX_FIELD.x, SIDE_GUTTER);
        assert_eq!(OMNIBOX_FIELD.y, FIELD_Y);
        assert_eq!(OMNIBOX_FIELD.h, FIELD_H);

        // The cluster ends on the trailing gutter the rest of the app keeps.
        let last = omnibox_tile(OMNIBOX_TRAILING - 1);
        assert_eq!(last.x + last.w, PANEL_W - SIDE_GUTTER);
        for i in 0..OMNIBOX_TRAILING {
            let tile = omnibox_tile(i);
            assert!(tile.w >= MIN_TARGET && tile.h >= MIN_TARGET, "tile {i}");
            assert!(tile.y >= OMNIBOX_Y0 && tile.y + tile.h <= OMNIBOX_Y1 - PROGRESS_H);
            assert!(
                tile.x >= OMNIBOX_FIELD.x + OMNIBOX_FIELD.w + TILE_GAP,
                "tile {i} stands on the field it trails"
            );
            if i > 0 {
                let prev = omnibox_tile(i - 1);
                assert_eq!(prev.x + prev.w + TILE_GAP, tile.x, "tile {i}");
            }
        }
    }

    /// The omnibox band is the platform's rows, and the field fills them.
    ///
    /// WHY derived: the band's bottom edge, the field's top and the plates' top
    /// were four literals (`116`, `56`, `60`, `52`) that only stayed correct while
    /// nobody edited the status bar or the touch floor. Every one is now a sum of
    /// the status band, the rung the address is set at and the platform's control
    /// floor, and this test walks the band the sum produces.
    #[test]
    fn the_omnibox_band_is_measured_off_the_platform_not_authored() {
        assert_eq!(
            OMNIBOX_Y0,
            interos_std::design::viewport::STATUS_BAR_HEIGHT
        );
        assert_eq!(OMNIBOX_Y1 - OMNIBOX_Y0, FIELD_H + 2 * OMNIBOX_DY);
        assert_eq!(FIELD_H, MIN_TARGET + spacing::GRID_8);
        // Every plate stops above the progress hairline the band also owns.
        assert!(OMNIBOX_FIELD.y + OMNIBOX_FIELD.h <= OMNIBOX_Y1 - PROGRESS_H);
        assert!(omnibox_tile(0).y + TILE <= OMNIBOX_Y1 - PROGRESS_H);
        assert_eq!(PROGRESS_Y0 + PROGRESS_H, OMNIBOX_Y1);
        assert!(PROGRESS_Y0 >= OMNIBOX_FIELD.y + OMNIBOX_FIELD.h);
    }

    /// The address run, its caret and its page mark all fit the field that
    /// carries them, with the same air above as below.
    ///
    /// WHY this is the descender gate for this row: the painter has no clip, so a
    /// run whose line box leaves the plate inks the band under it, and a run
    /// centred by a remembered offset rather than by its own box leaves one side
    /// tighter than the other -- the address used to sit at row `70` of a plate
    /// running `56..108`, fourteen rows of air over eleven.
    #[test]
    fn the_address_run_fits_the_field_that_carries_it() {
        let line_box = font::line_height(FIELD_RUNG);
        assert!(FIELD_TEXT_Y >= FIELD_Y);
        assert!(FIELD_TEXT_Y + line_box <= FIELD_Y + FIELD_H);
        assert_eq!(
            FIELD_TEXT_Y - FIELD_Y,
            FIELD_Y + FIELD_H - (FIELD_TEXT_Y + line_box),
            "the address is not centred in its own field"
        );

        // The run's lane starts past the mark and ends inside the plate.
        assert!(FIELD_TEXT_X >= OMNIBOX_FIELD.x + FIELD_MARK_DX + FIELD_MARK_SIDE);
        assert!(FIELD_TEXT_X + FIELD_TEXT_LANE <= OMNIBOX_FIELD.x + OMNIBOX_FIELD.w);
        assert!(FIELD_TEXT_LANE >= MIN_TARGET, "the lane cannot hold an address");

        // The page mark is centred on the same plate, and the run starts after it.
        assert_eq!(
            FIELD_MARK_Y + FIELD_MARK_SIDE / 2,
            FIELD_Y + FIELD_H / 2,
            "the mark is off the field's own middle"
        );
        assert!(FIELD_MARK_Y >= FIELD_Y);
        assert!(FIELD_MARK_Y + FIELD_MARK_SIDE <= FIELD_Y + FIELD_H);
        assert_eq!(FIELD_TEXT_DX, FIELD_MARK_DX + FIELD_MARK_SIDE + spacing::GRID_8);
    }

    /// Every control box in this module clears the platform's touch floor.
    ///
    /// WHY a table and not one test per box: the floor is one rule, and a rule
    /// checked in twelve places is twelve places that can be edited apart. A box
    /// that joins this module without joining this table is not gating the row
    /// the person is asked to press.
    #[test]
    fn every_chrome_control_clears_the_platform_touch_floor() {
        let boxes: [(&str, Rect); 18] = [
            ("sheet close", SHEET_CLOSE),
            ("filter card", FILTER_CARD),
            ("filter clear", FILTER_CLEAR),
            ("retry", retry_rect(SHEET_HEADER_Y0)),
            ("retry at the longest message", retry_rect(672)),
            ("suggestion search", SUGGEST_SEARCH),
            ("suggestion navigate", SUGGEST_NAVIGATE),
            ("list row", list_row_card(FILTER_Y + FILTER_H, 62)),
            ("accessory https", ACC_HTTPS),
            ("accessory .com", ACC_TLD_COM),
            ("accessory .org", ACC_TLD_ORG),
            ("accessory clear", ACC_CLEAR),
            ("accessory dismiss", ACC_DISMISS),
            ("field clear", FIELD_CLEAR),
            ("field done", FIELD_DONE),
            ("omnibox field", OMNIBOX_FIELD),
            ("omnibox reload", omnibox_tile(0)),
            ("omnibox menu", omnibox_tile(1)),
        ];
        for (name, r) in boxes {
            assert!(r.w >= MIN_TARGET, "{name} is {} wide", r.w);
            assert!(r.h >= MIN_TARGET, "{name} is {} tall", r.h);
            assert!(r.x + r.w <= PANEL_W, "{name} overhangs the panel's edge");
        }
    }

    /// The four start-page tiles answer their plate and their caption, and
    /// never each other's columns.
    #[test]
    fn a_quick_link_tile_answers_its_plate_and_its_caption() {
        for i in 0..HOME_TILE_X.len() {
            let hit = home_tile_hit(i);
            // The plate the renderer draws sits inside the band, so no pixel of
            // the control is unreachable.
            assert!(
                hit.x <= HOME_TILE_X[i] && hit.x + hit.w >= HOME_TILE_X[i] + HOME_TILE_SIZE,
                "tile {i} does not answer its own plate"
            );
            assert!(
                hit.y <= HOME_TILE_Y && hit.y + hit.h > HOME_TILE_CAPTION_Y,
                "tile {i} does not answer the caption that names it"
            );
            assert!(hit.w >= MIN_TARGET && hit.h >= MIN_TARGET, "tile {i}");
        }
        for i in 1..HOME_TILE_X.len() {
            let prev = home_tile_hit(i - 1);
            assert!(prev.x + prev.w <= home_tile_hit(i).x);
        }
    }

    /// A tile's caption is answered across its whole run, not just its first
    /// column.
    ///
    /// WHY measured against the face: the const gate beside the table can only
    /// see where a caption starts. How far it runs is the typeface's business,
    /// and a word that ran past its band would be a label the neighbouring tile
    /// answers -- the painted-versus-tappable defect wearing a second hat.
    #[test]
    fn a_quick_link_caption_is_answered_across_its_whole_run() {
        for i in 0..HOME_TILE_X.len() {
            let hit = home_tile_hit(i);
            let run = interos_std::design::font::measure(HOME_TILE_CAPTION[i], 1);
            assert!(run > 0, "tile {i} paints no caption at all");
            assert!(
                HOME_TILE_CAPTION_X[i] + run <= hit.x + hit.w,
                "tile {i}'s caption runs past its band by {} columns",
                HOME_TILE_CAPTION_X[i] + run - (hit.x + hit.w)
            );
            assert!(
                HOME_TILE_CAPTION_X[i] >= hit.x,
                "tile {i}'s caption starts outside the band that answers it"
            );
        }
    }

    /// The reload action answers exactly the plate the error screen draws.
    ///
    /// WHY swept over every badge row: the badge's row is a function of how many
    /// lines the failure message wrapped to, and the defect this gate holds shut
    /// was a fixed band that answered only one of those rows.
    #[test]
    fn the_reload_action_answers_the_plate_at_every_message_length() {
        for body_n in 1..=6usize {
            let err_y = 440 + 40 + body_n * 28 + 24;
            let retry = retry_rect(err_y);
            assert_eq!(retry.y, err_y + ACTION_DY);
            assert_eq!(retry.h, ACTION_H);
            // A press on any row of the drawn button is answered...
            assert!(retry.contains(retry.x, retry.y));
            assert!(retry.contains(retry.x + retry.w - 1, retry.y + retry.h - 1));
            // ...and a press just outside it is not.
            assert!(!retry.contains(retry.x, retry.y - 1));
            assert!(!retry.contains(retry.x, retry.y + retry.h));
            assert!(!retry.contains(retry.x - 1, retry.y));
            assert!(!retry.contains(retry.x + retry.w, retry.y));
        }
    }

    /// Every sheet's close control is drawn where it answers.
    #[test]
    fn the_sheet_close_control_is_painted_where_it_answers() {
        assert!(!SHEET_CLOSE.contains(back_button::X, back_button::Y));
        // The plate is the SDK's own chrome tile: 44 rows inside a 50-row
        // header band, so the band cannot be edited out from under it.
        assert_eq!(SHEET_CLOSE.w, MIN_TARGET);
        assert_eq!(SHEET_CLOSE.h, MIN_TARGET);
        assert_eq!(
            SHEET_CLOSE.x + SHEET_CLOSE.w,
            PANEL_W - SIDE_GUTTER,
            "the close control keeps the sheet's own gutter"
        );
        for y in SHEET_CLOSE.y..SHEET_CLOSE.y + SHEET_CLOSE.h {
            assert!(y >= SHEET_HEADER_Y0 && y < SHEET_HEADER_Y0 + SHEET_HEADER_H);
        }
    }

    /// The filter's clear control answers its plate and none of the card's empty
    /// padding, and stays inside the card it is drawn on.
    #[test]
    fn the_filter_clear_answers_its_own_plate_only() {
        assert_eq!(FILTER_CLEAR.w, MIN_TARGET);
        assert_eq!(FILTER_CLEAR.h, MIN_TARGET);
        // Trailing half of the card, which is where a clear control belongs.
        assert!(FILTER_CLEAR.x > FILTER_CARD.x + FILTER_CARD.w / 2);
        // Card left around the plate: its own rounding, plus the gutter.
        assert!(FILTER_CARD.x + FILTER_CARD.w - (FILTER_CLEAR.x + FILTER_CLEAR.w) >= 8);
        assert!(FILTER_CLEAR.y - FILTER_CARD.y >= 4);
        assert!(FILTER_CARD.y + FILTER_CARD.h - (FILTER_CLEAR.y + FILTER_CLEAR.h) >= 4);
    }

    /// The accessory strip's chips tile its plate without overlapping, and the
    /// strip's own band is the plate the renderer fills.
    #[test]
    fn the_accessory_chips_answer_their_plates_and_not_the_strip() {
        let chips = [
            &ACC_HTTPS,
            &ACC_TLD_COM,
            &ACC_TLD_ORG,
            &ACC_CLEAR,
            &ACC_DISMISS,
        ];
        for chip in chips {
            assert_eq!(chip.y, ACC_CHIP_Y);
            assert_eq!(chip.h, ACC_CHIP_H);
            assert!(chip.y + chip.h <= ACC_BAR_Y1);
        }
        assert!(ACC_HTTPS.x + ACC_HTTPS.w <= ACC_TLD_COM.x);
        assert!(ACC_TLD_COM.x + ACC_TLD_COM.w <= ACC_TLD_ORG.x);
        assert!(ACC_TLD_ORG.x + ACC_TLD_ORG.w <= ACC_CLEAR.x);
        assert!(ACC_CLEAR.x + ACC_CLEAR.w <= ACC_DISMISS.x);

        // The focused-field pair sits in the same band and clears each other.
        assert!(FIELD_CLEAR.x + FIELD_CLEAR.w <= FIELD_DONE.x);

        // The strip's plate ends where the band tested by the press path ends,
        // so the row under the plate is not the strip's to answer.
        assert_eq!(ACC_BAR_Y1, ACC_BAR_Y0 + ACC_BAR_H);
        assert!(!ACC_HTTPS.contains(ACC_HTTPS.x, ACC_BAR_Y1));
    }
}
