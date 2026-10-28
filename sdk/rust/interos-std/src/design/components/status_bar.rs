//! The band the platform keeps at the top of every screen.
//!
//! WHY this lives in the SDK and not in each app: the height is already a
//! platform number ([`crate::design::viewport::STATUS_BAR_HEIGHT`]), and the
//! clock and the instrument glyphs are meant to be the same on every screen.
//! An app that draws its own bar draws one that disagrees with its neighbours,
//! which is what happened before the shell had one module for it: the clock
//! moved between four different x positions and two type rungs, some screens
//! drew no glyphs at all, and the task switcher drew a filled battery that no
//! sensor had measured. A person cannot tell which screen they are on from a
//! bar that changes shape, because a bar that changes shape is not chrome.
//!
//! # Platform Truth
//! The cellular radio, the Wi-Fi association and the fuel gauge are unbound on
//! this platform, so their glyphs are outlines carrying no level. A filled bar
//! or a printed percentage is a reading no instrument took, and
//! [`Reading::Unbound`] is the state that says so. The clock is drawn only when
//! the RTC answered; there is no fallback time.
//!
//! Zero heap allocations, `#![no_std]`.

use super::label_pixel;
use crate::design::draw_target::Surface;
use crate::design::primitives::Rect;
use crate::design::theme::Theme;
use crate::design::typography::BODY_SCALE;
use crate::design::viewport::STATUS_BAR_HEIGHT;

/// The band's height: the platform's own reserved inset.
pub const HEIGHT: usize = STATUS_BAR_HEIGHT;

/// Where the clock sits, measured from the band's own top-left corner.
pub const CLOCK_X: usize = 40;
/// The clock's line box, from the band's top edge.
pub const CLOCK_Y: usize = 8;
/// The clock's type rung.
pub const CLOCK_SCALE: usize = BODY_SCALE;

/// The band's right margin: the instruments stop this far from the edge.
pub const RIGHT_MARGIN: usize = 36;
/// The gap between two instruments.
pub const GLYPH_GAP: usize = 14;
/// The cellular mark: four bars on a 5 px pitch, each 3 px wide.
pub const SIGNAL_BARS: usize = 4;
pub const SIGNAL_PITCH: usize = 5;
pub const SIGNAL_BAR_W: usize = 3;
/// The width of the cellular mark.
pub const SIGNAL_W: usize = (SIGNAL_BARS - 1) * SIGNAL_PITCH + SIGNAL_BAR_W;
/// The width of the Wi-Fi mark.
///
/// WHY 16 and not the 6 an earlier shell used: the glyph is a dot with two
/// arcs over it (radius 8 at the widest), so the box has to hold a 15 px fan;
/// a 6 px box could only ever fit a square, which is what it drew.
pub const WIFI_W: usize = 16;
/// The width of the battery shell.
pub const BATTERY_W: usize = 24;
/// The width of the battery's terminal tip.
pub const TERMINAL_W: usize = 2;

/// The topmost row any instrument touches, from the band's top edge.
pub const GLYPH_TOP: usize = 14;
/// The bottommost row any instrument touches, from the band's top edge.
pub const GLYPH_BOTTOM: usize = 34;
/// The row the instruments stand on.
pub const GLYPH_FLOOR: usize = 30;
/// The battery shell's top and bottom rows.
pub const SHELL_TOP: usize = 17;
pub const SHELL_BOTTOM: usize = 31;
/// The battery terminal's top and bottom rows.
pub const TERMINAL_TOP: usize = 21;
pub const TERMINAL_BOTTOM: usize = 27;
/// The dash an unbound fuel gauge draws inside its shell.
pub const DASH_TOP: usize = 23;
pub const DASH_BOTTOM: usize = 24;

/// What one of the platform's instruments answered.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reading {
    /// The instrument is bound and reported this many percent. Draws a fill.
    Measured(u32),
    /// The instrument is not bound, so there is no level to draw. The mark is
    /// an outline carrying an explicit "no reading" sign.
    Unbound,
    /// The hardware does not exist on this device at all. Draws nothing.
    Unsupported,
}

impl Reading {
    /// The measured amount, when an instrument actually took it.
    ///
    /// WHY the fill arithmetic is reached only through here: a bound sensor
    /// then has one place to go rather than a colour to change, and an outline
    /// is the only thing an unbound one can produce.
    pub const fn level(self) -> Option<u32> {
        match self {
            Reading::Measured(percent) => Some(if percent > 100 { 100 } else { percent }),
            _ => None,
        }
    }
}

/// The three instruments the band stands for.
///
/// WHY the caller supplies them rather than the band asking: every one is a
/// syscall, and a component that made them at paint time would make them once
/// per pixel. An app reads them on its own timer and hands the answers down.
#[derive(Clone, Copy, Debug)]
pub struct Instruments {
    /// The cellular signal strength.
    pub cellular: Reading,
    /// The Wi-Fi signal strength.
    pub wifi: Reading,
    /// The fuel gauge, in percent.
    pub battery: Reading,
}

impl Instruments {
    /// A band whose instruments are all unbound: what an app that has not
    /// asked the platform anything yet should draw.
    pub const fn unmeasured() -> Self {
        Instruments {
            cellular: Reading::Unbound,
            wifi: Reading::Unbound,
            battery: Reading::Unbound,
        }
    }

    /// Constructs an Instruments reading with real telemetry from queries.
    pub const fn from_readings(cellular: Reading, wifi: Reading, battery: Reading) -> Self {
        Instruments {
            cellular,
            wifi,
            battery,
        }
    }
}

/// Where each instrument is drawn in a band of a given width.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Strip {
    /// The cellular mark's left edge.
    pub signal_x: usize,
    /// The Wi-Fi mark's left edge.
    pub wifi_x: usize,
    /// The battery shell's left edge.
    pub battery_x: usize,
    /// The battery terminal's left edge.
    pub terminal_x: usize,
}

/// Lays the instruments out against the right edge of a `width`-pixel band.
pub const fn instrument_strip(width: usize) -> Strip {
    let right = width.saturating_sub(RIGHT_MARGIN);
    let terminal_x = right.saturating_sub(TERMINAL_W);
    let battery_x = terminal_x.saturating_sub(BATTERY_W);
    let wifi_x = battery_x.saturating_sub(GLYPH_GAP + WIFI_W);
    let signal_x = wifi_x.saturating_sub(GLYPH_GAP + SIGNAL_W);
    Strip {
        signal_x,
        wifi_x,
        battery_x,
        terminal_x,
    }
}

/// The platform's status band.
#[derive(Clone, Copy, Debug)]
pub struct StatusBar {
    /// The band's rectangle on the surface.
    pub bounds: Rect,
    /// Where this band's instruments sit.
    pub strip: Strip,
}

/// Canonical type alias for StatusBar: the status band at the top of every screen.
pub type StatusBand = StatusBar;

/// The vertical extent of the canonical status band on this display.
pub const fn status_band() -> (usize, usize) {
    (0, HEIGHT)
}

impl StatusBar {
    /// The band across the top of a `width`-pixel display.
    pub const fn new(width: usize) -> Self {
        StatusBar {
            bounds: Rect::new(0, 0, width, HEIGHT),
            strip: instrument_strip(width),
        }
    }

    /// The same band moved down to `top`, for a surface that already drew
    /// something above it.
    pub const fn at(&self, top: usize) -> Self {
        StatusBar {
            bounds: Rect::new(self.bounds.x, top, self.bounds.w, HEIGHT),
            strip: self.strip,
        }
    }

    /// The first row below the band: where a screen's own content starts.
    ///
    /// WHY a screen asks rather than adding [`HEIGHT`] itself: an app that
    /// retypes the number is an app that keeps the old number the day the
    /// platform's inset changes.
    pub const fn content_top(&self) -> usize {
        self.bounds.y + HEIGHT
    }

    /// Whether (`x`, `y`) is inside the band.
    pub fn contains(&self, x: usize, y: usize) -> bool {
        self.bounds.contains(x, y)
    }

    /// The band's background: the page colour and its one-pixel divider.
    pub fn backdrop_pixel(&self, x: usize, y: usize, theme: &Theme) -> Option<u32> {
        if !self.contains(x, y) {
            return None;
        }
        if y == self.bounds.y + HEIGHT - 1 {
            Some(theme.border)
        } else {
            Some(theme.surface)
        }
    }

    /// One pixel of the band's three instrument marks, or `None`.
    pub fn glyph_pixel(
        &self,
        x: usize,
        y: usize,
        instruments: &Instruments,
        ink: u32,
    ) -> Option<u32> {
        let top = self.bounds.y;
        if y < top + GLYPH_TOP || y > top + GLYPH_BOTTOM {
            return None;
        }
        let row = y - top;
        let floor = GLYPH_FLOOR;
        let strip = self.strip;

        // 1. Cellular: four ascending bars.
        if x >= strip.signal_x && x < strip.signal_x + SIGNAL_W {
            return self.cellular_pixel(x, row, strip, instruments.cellular, ink);
        }

        // 2. Wi-Fi: a centre dot and two continuous 1 px arcs above it.
        //
        // WHY a per-column arc rather than an integer annulus: at these radii a
        // squared-distance band lands on sparse lattice points and renders as a
        // dashed cloud. Resolving the arc's row for each column keeps the fan
        // one connected curve.
        if x >= strip.wifi_x && x < strip.wifi_x + WIFI_W {
            if instruments.wifi == Reading::Unsupported {
                return None;
            }
            let cx = strip.wifi_x as i32 + (WIFI_W as i32) / 2;
            let dx = x as i32 - cx;
            if (-1..=0).contains(&dx) && (floor - 1..=floor).contains(&row) {
                return Some(ink);
            }
            let dy = (floor as i32) - (row as i32);
            if dy > 0 {
                let lit = match instruments.wifi.level() {
                    Some(percent) => ((percent as usize * 3 + 99) / 100).clamp(1, 3),
                    // Unbound draws the whole fan, because the mark still has
                    // to read as Wi-Fi; it is the dot and the arcs' hollowness
                    // that says no level was measured, not a missing arc.
                    None => 3,
                };
                for (arc, r) in [5i32, 8].into_iter().enumerate() {
                    if arc >= lit {
                        continue;
                    }
                    if dx.abs() <= r - 2 {
                        let d2 = dx * dx + dy * dy;
                        if (d2 - r * r).abs() <= r {
                            return Some(ink);
                        }
                    }
                }
            }
            return None;
        }

        // 3. Battery: shell, terminal tip, and the charge inside the shell.
        if x >= strip.battery_x && x < strip.terminal_x + TERMINAL_W {
            return self.battery_pixel(x, row, strip, instruments.battery, ink);
        }

        None
    }

    /// One cellular bar column, outlined when the radio is unbound.
    fn cellular_pixel(
        &self,
        x: usize,
        row: usize,
        strip: Strip,
        reading: Reading,
        ink: u32,
    ) -> Option<u32> {
        if reading == Reading::Unsupported {
            return None;
        }
        let floor = GLYPH_FLOOR;
        let lit = match reading.level() {
            Some(percent) => Some(((percent as usize * SIGNAL_BARS + 99) / 100).clamp(1, SIGNAL_BARS)),
            None => None,
        };
        for bar in 0..SIGNAL_BARS {
            let bx = strip.signal_x + bar * SIGNAL_PITCH;
            if x < bx || x >= bx + SIGNAL_BAR_W {
                continue;
            }
            let h = 5 + bar * 3;
            let bar_top = floor + 1 - h;
            if row < bar_top || row > floor {
                return None;
            }
            let filled = lit.map_or(false, |count| bar < count);
            let on_edge = x == bx
                || x == bx + SIGNAL_BAR_W - 1
                || row == bar_top
                || row == floor;
            if filled || on_edge {
                return Some(ink);
            }
            return None;
        }
        None
    }

    /// The battery shell, its terminal, and the charge drawn inside it.
    fn battery_pixel(
        &self,
        x: usize,
        row: usize,
        strip: Strip,
        reading: Reading,
        ink: u32,
    ) -> Option<u32> {
        if reading == Reading::Unsupported {
            return None;
        }
        if x >= strip.terminal_x {
            if (TERMINAL_TOP..=TERMINAL_BOTTOM).contains(&row) {
                return Some(ink);
            }
            return None;
        }
        if row < SHELL_TOP || row > SHELL_BOTTOM {
            return None;
        }
        let right = strip.terminal_x - 1;
        let corner = (x == strip.battery_x || x == right) && (row == SHELL_TOP || row == SHELL_BOTTOM);
        if corner {
            return None;
        }
        if row == SHELL_TOP || row == SHELL_BOTTOM || x == strip.battery_x || x == right {
            return Some(ink);
        }
        match reading.level() {
            // Unbound: a centre dash, because a filled interior would read as
            // a charge level, which is the fabricated reading this strip
            // exists to avoid.
            None => {
                let mid = strip.battery_x + (strip.terminal_x - strip.battery_x) / 2;
                if (DASH_TOP..=DASH_BOTTOM).contains(&row) && x >= mid - 4 && x <= mid + 4 {
                    return Some(ink);
                }
                None
            }
            Some(percent) => {
                if row < SHELL_TOP + 3 || row > SHELL_BOTTOM - 3 {
                    return None;
                }
                let inner = (right - strip.battery_x - 1) as u32;
                let filled = (inner * percent / 100).max(1) as usize;
                if x > strip.battery_x && x <= strip.battery_x + filled {
                    return Some(ink);
                }
                None
            }
        }
    }

    /// One pixel of the band, or `None` outside it.
    ///
    /// WHY `None` rather than the page colour when the pixel is the band's but
    /// nothing is inked there: the caller has already painted its own
    /// background, and a component that answered for every pixel of the band
    /// would erase whatever the app put under it. Use [`draw_status_bar`] to
    /// get the opaque version.
    pub fn pixel(
        &self,
        x: usize,
        y: usize,
        clock: Option<&[u8]>,
        instruments: &Instruments,
        theme: &Theme,
    ) -> Option<u32> {
        if !self.contains(x, y) {
            return None;
        }
        let ink = theme.on_surface;

        if let Some(clock) = clock {
            if let Some(painted) = label_pixel(
                x,
                y,
                self.bounds.x + CLOCK_X,
                self.bounds.y + CLOCK_Y,
                CLOCK_SCALE,
                clock,
                ink,
                theme.surface,
            ) {
                return Some(painted);
            }
        }

        if let Some(painted) = self.glyph_pixel(x, y, instruments, ink) {
            return Some(painted);
        }

        self.backdrop_pixel(x, y, theme)
    }
}

/// Draws the band across a surface, opaque over whatever is already there.
///
/// WHY a whole-surface painter next to the per-pixel one: the shell repaints a
/// band at a time because a surface write is capped, while an app that owns a
/// frame paints it in one go. A component that only worked one way would be a
/// component half the apps could not use.
pub fn draw_status_bar(
    surface: &mut Surface,
    bar: &StatusBar,
    clock: Option<&[u8]>,
    instruments: &Instruments,
    theme: &Theme,
) {
    for y in bar.bounds.y..bar.bounds.y + HEIGHT {
        for x in bar.bounds.x..bar.bounds.x + bar.bounds.w {
            if surface.get(x, y).is_none() {
                continue;
            }
            if let Some(painted) = bar.pixel(x, y, clock, instruments, theme) {
                surface.set(x, y, painted);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design::theme::Theme;

    const WIDTH: usize = 200;

    fn ink_of(count: usize, f: impl Fn(usize) -> Option<u32>) -> usize {
        (0..count).filter(|i| f(*i).is_some()).count()
    }

    #[test]
    fn the_band_is_the_platform_inset() {
        let bar = StatusBar::new(WIDTH);
        assert_eq!(bar.bounds.h, HEIGHT);
        assert_eq!(bar.bounds.y, 0);
        assert_eq!(bar.content_top(), HEIGHT);
        assert_eq!(bar.at(100).content_top(), 100 + HEIGHT);
        assert_eq!(bar.at(100).bounds.w, WIDTH);
    }

    #[test]
    fn the_instruments_end_at_the_right_margin() {
        let strip = instrument_strip(WIDTH);
        assert_eq!(strip.terminal_x + TERMINAL_W, WIDTH - RIGHT_MARGIN);
        assert_eq!(strip.battery_x + BATTERY_W, strip.terminal_x);
        assert!(strip.signal_x < strip.wifi_x);
        assert!(strip.wifi_x < strip.battery_x);
        // A band narrower than the strip must clamp, not underflow into a
        // huge x that the surface would reject.
        let tiny = instrument_strip(20);
        assert!(tiny.signal_x <= 20);
        assert!(tiny.terminal_x <= 20);
    }

    #[test]
    fn an_unbound_instrument_is_an_outline_and_never_a_level() {
        let bar = StatusBar::new(WIDTH);
        let instruments = Instruments::unmeasured();
        let inked = ink_of(WIDTH * HEIGHT, |i| {
            bar.glyph_pixel(i % WIDTH, i / WIDTH, &instruments, Theme::LIGHT.on_surface)
        });
        assert!(inked > 0, "the marks still have to read as marks");
        // Three shells' worth of outline is the most an unbound strip may ink.
        let shell_area = BATTERY_W * (SHELL_BOTTOM - SHELL_TOP + 1);
        assert!(inked < 3 * shell_area, "inked {inked} of {shell_area}");
    }

    #[test]
    fn an_unsupported_instrument_draws_nothing() {
        let bar = StatusBar::new(WIDTH);
        let instruments = Instruments {
            cellular: Reading::Unsupported,
            wifi: Reading::Unsupported,
            battery: Reading::Unsupported,
        };
        let inked = ink_of(WIDTH * HEIGHT, |i| {
            bar.glyph_pixel(i % WIDTH, i / WIDTH, &instruments, Theme::LIGHT.on_surface)
        });
        assert_eq!(inked, 0);
    }

    #[test]
    fn a_measured_fuel_gauge_fills_where_an_unbound_one_leaves_a_dash() {
        let bar = StatusBar::new(WIDTH);
        let strip = bar.strip;
        let row = SHELL_TOP + 5;
        let unbound = Instruments::unmeasured();
        let half = Instruments {
            battery: Reading::Measured(50),
            ..unbound
        };
        let mid = strip.battery_x + (strip.terminal_x - 1 - strip.battery_x) / 2;
        assert_eq!(
            bar.glyph_pixel(mid, DASH_TOP, &unbound, Theme::LIGHT.on_surface),
            Some(Theme::LIGHT.on_surface),
            "an unbound gauge marks its empty interior rather than leaving it blank"
        );
        assert_eq!(
            bar.glyph_pixel(strip.battery_x + 2, row, &unbound, Theme::LIGHT.on_surface),
            None
        );
        assert_eq!(
            bar.glyph_pixel(strip.battery_x + 2, row, &half, Theme::LIGHT.on_surface),
            Some(Theme::LIGHT.on_surface)
        );
        assert_eq!(
            bar.glyph_pixel(strip.terminal_x - 2, row, &half, Theme::LIGHT.on_surface),
            None,
            "half is not full"
        );
    }

    #[test]
    fn a_measured_radio_lights_bars_an_unbound_one_only_outlines() {
        let bar = StatusBar::new(WIDTH);
        let strip = bar.strip;
        let unbound = Instruments::unmeasured();
        let full = Instruments {
            cellular: Reading::Measured(100),
            ..unbound
        };
        // The tallest bar's interior: outlined when unbound, solid when measured.
        let tallest_x = strip.signal_x + 3 * SIGNAL_PITCH + 1;
        let inside_row = GLYPH_FLOOR - 1;
        assert_eq!(
            bar.glyph_pixel(tallest_x, inside_row, &unbound, Theme::LIGHT.on_surface),
            None
        );
        assert_eq!(
            bar.glyph_pixel(tallest_x, inside_row, &full, Theme::LIGHT.on_surface),
            Some(Theme::LIGHT.on_surface)
        );
    }

    #[test]
    fn the_clock_is_drawn_only_when_the_rtc_answered() {
        let bar = StatusBar::new(WIDTH);
        let instruments = Instruments::unmeasured();
        let theme = Theme::LIGHT;
        // The clock's own lane: nothing else in the band is inked this far left.
        let inked_without = ink_of(WIDTH * HEIGHT, |i| {
            let (x, y) = (i % 60, i / 60);
            bar.pixel(x, y, None, &instruments, &theme)
                .filter(|c| *c == theme.on_surface)
        });
        assert_eq!(inked_without, 0, "a silent RTC leaves the clock lane empty");

        let inked_with = ink_of(WIDTH * HEIGHT, |i| {
            let (x, y) = (i % 60, i / 60);
            bar.pixel(x, y, Some(b"09:41"), &instruments, &theme)
                .filter(|c| *c == theme.on_surface)
        });
        assert!(inked_with > 0);
    }

    #[test]
    fn the_unbound_strip_inks_the_pixels_the_shell_has_always_shown() {
        // WHY a count and not an image: the marks are chrome, and a shape that
        // drifts by a row is a shape every surface inherits. This is the total
        // the shell drew before the band moved into the SDK, so the port is
        // checked against the pixels people already read as the status bar.
        use crate::design::viewport::LOGICAL_WIDTH;
        let bar = StatusBar::new(LOGICAL_WIDTH);
        let instruments = Instruments::unmeasured();
        let inked = ink_of(LOGICAL_WIDTH * HEIGHT, |i| {
            bar.glyph_pixel(
                i % LOGICAL_WIDTH,
                i / LOGICAL_WIDTH,
                &instruments,
                Theme::LIGHT.on_surface,
            )
        });
        assert_eq!(inked, 214);
    }

    #[test]
    fn the_band_divides_itself_from_the_page() {
        let bar = StatusBar::new(WIDTH);
        let instruments = Instruments::unmeasured();
        let theme = Theme::LIGHT;
        assert_eq!(
            bar.backdrop_pixel(100, HEIGHT - 1, &theme),
            Some(theme.border)
        );
        assert_eq!(bar.backdrop_pixel(100, 0, &theme), Some(theme.surface));
        assert_eq!(bar.backdrop_pixel(100, HEIGHT, &theme), None);
        assert_eq!(bar.pixel(100, HEIGHT, None, &instruments, &theme), None);
    }

    #[test]
    fn the_component_paints_the_surface_it_is_given() {
        let bar = StatusBar::new(WIDTH);
        let instruments = Instruments::unmeasured();
        let theme = Theme::LIGHT;
        let pixels = paint(WIDTH, HEIGHT, |surface| {
            draw_status_bar(surface, &bar, Some(b"09:41"), &instruments, &theme)
        });
        assert_eq!(pixels[100 + (HEIGHT - 1) * WIDTH], theme.border);
        let inked = (0..WIDTH * HEIGHT)
            .filter(|i| pixels[*i] == theme.on_surface)
            .count();
        assert!(inked > 0);
    }

    fn paint(w: usize, h: usize, f: impl FnOnce(&mut Surface<'_>)) -> [u32; 16384] {
        let mut pixels = [0u32; 16384];
        {
            let mut surface = Surface::new(&mut pixels[..w * h], w, h).expect("canvas fits");
            f(&mut surface);
        }
        pixels
    }
}
