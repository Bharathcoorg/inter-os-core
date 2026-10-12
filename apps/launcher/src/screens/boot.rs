//! Guest-owned startup presentation rendered into the native portrait surface.
//! This is the post-userspace boot animation; bootloader/kernel output remains
//! guest-owned.
//!
//! WHY this is one animation and not a slideshow: a first boot is a single
//! gesture -- the mark ignites, the name opens out of it, and the device then
//! says "working" for exactly as long as it works. The mark, the wordmark and
//! the sweep are one object rather than three things on a background: the orb is
//! the body, the sweep is the body's own light circling it, and the wordmark is
//! the body's name, arriving through the same reveal that grows the body.
//!
//! WHY nothing here states a percentage: `boot_progress` counts timer ticks
//! against the presentation window `main.rs` hands this view, so it measures
//! elapsed time and nothing else. No step of the boot has reported completion to
//! this surface, so drawing that counter as a bar would claim "this much of the
//! boot is done" about a number that has never read one. The sweep below is
//! therefore indeterminate by construction -- it turns at a constant rate, it
//! never accumulates, and it states no fraction. That is honest behaviour rather
//! than an honest label, and it is pinned by a test.
//!
//! WHY it is lean: the surface is 720x1440 and `main.rs` repaints 350 of those
//! rows per tick under TCG, so this painter is a handful of integer tests per
//! pixel. There is no square root on the path (the sphere's height is a walk
//! bounded by its radius), no trig (the turn is a table), and the outline
//! wordmark is placed once for the life of the surface rather than once per
//! pixel.

use core::cell::UnsafeCell;

use crate::{LauncherState, H, W};
use interos_std::design::spacing;
use launcher::outline_text::{self, OutlineRun};
use launcher::theme::Theme;

/// The theme this surface is drawn in.
///
/// WHY a constant and not a runtime choice: this presentation runs before the
/// settings store has answered anything, so there is no theme to read. The dark
/// system is what a first boot looks like on this device, and every colour below
/// comes from it -- this file carries no colour literal of its own.
const THEME: Theme = Theme::DARK;

// -----------------------------------------------------------------------------
// The band this view owns
// -----------------------------------------------------------------------------

/// Rows `main.rs` repaints while this view is up. It calls
/// `render_band(surface, state, H / 2 - 130, H / 2 + 220)`, so these mirror that
/// band: nothing above it or below it is ever repainted by this view, and every
/// element below is placed inside it. If the call site moves, these move.
const BAND_TOP: usize = H / 2 - 130;
const BAND_ROWS: usize = 350;

// -----------------------------------------------------------------------------
// The mark
// -----------------------------------------------------------------------------

/// Centre of the mark: horizontally on the surface, vertically so the group the
/// sweep draws around it sits evenly above the wordmark.
const BRAND_CX: usize = W / 2;
const BRAND_CY: usize = WORDMARK_TOP - spacing::XXL - SWEEP_R;

/// Resting radius of the mark's body.
///
/// WHY 34: the brand mark is the small end of the body the System Space orb
/// grows into, and that sphere is 52. A third of it is small enough to sit under
/// a 51px wordmark without competing with it, and large enough that the
/// highlight and the terminator are still resolvable after Device Studio's
/// 0.457x presentation scale.
const BODY_R: usize = 34;

/// Radius the sweep travels on, outboard of the body.
///
/// WHY 58 and not closer: the stroke is 4 rows, so a 56..60 annulus leaves 20
/// rows of backdrop between the body's silhouette and the sweep. That gap is
/// what makes the two read as one object with the light around it, rather than
/// as a ring glued to a disc.
const SWEEP_R: usize = 58;

/// Half the sweep stroke's width, in rows.
const SWEEP_STROKE: i32 = 2;

/// The mark's resting light angle: high, to the left and toward the viewer, in
/// the System Space orb's own units.
///
/// WHY these exact numbers: the light is what makes two spheres the same
/// sphere. Sharing the direction and the rotation is what lets a user read this
/// mark and that orb as one body, so the direction is carried rather than
/// re-chosen.
const LIGHT_X: i32 = -3;
const LIGHT_Y: i32 = -4;
const LIGHT_Z: i32 = 4;

/// How much brighter this lamp runs than that one's.
///
/// WHY it is not the same magnitude: that screen's lamp is tuned for a 52-row
/// body, where the diffuse ramp spreads over enough pixels to resolve. At 34 rows
/// the same magnitude collapses the ramp onto its lowest step -- the mark would
/// be technically the same shading and identically blank, a flat disc with a rim.
/// Six times the magnitude restores the gradient at this size without moving the
/// light or changing its rotation, so the direction and the turn are that orb's
/// and the brightness is this mark's.
const LAMP_GAIN: i32 = 6;
/// Fixed-point scale of the turn table below, in 64ths.
const TRIG_SCALE: i32 = 64;

/// One turn in 32 steps, as `(dx, dy)` in 64ths with step 0 at the top of the
/// surface and the step number increasing clockwise.
///
/// WHY a table and not trig: this target has no floats and no libm, and the
/// sweep needs a direction for every frame. 32 steps over a turn is 11.25
/// degrees, which at the sweep's radius is 11 rows of travel -- coarser than a
/// 4-row stroke can resolve, so the table costs nothing on screen. The same
/// table also rotates the mark's light, so the orb's highlight and the sweep's
/// head share one clock and cannot drift apart.
const TURN_STEPS: usize = 32;
const TURN: [(i32, i32); TURN_STEPS] = [
    (0, -64),
    (12, -63),
    (24, -59),
    (36, -53),
    (45, -45),
    (53, -36),
    (59, -24),
    (63, -12),
    (64, 0),
    (63, 12),
    (59, 24),
    (53, 36),
    (45, 45),
    (36, 53),
    (24, 59),
    (12, 63),
    (0, 64),
    (-12, 63),
    (-24, 59),
    (-36, 53),
    (-45, 45),
    (-53, 36),
    (-59, 24),
    (-63, 12),
    (-64, 0),
    (-63, -12),
    (-59, -24),
    (-53, -36),
    (-45, -45),
    (-36, -53),
    (-24, -59),
    (-12, -63),
];

// -----------------------------------------------------------------------------
// The one reveal
// -----------------------------------------------------------------------------

/// Ticks the reveal takes, from a point to the resting size.
///
/// WHY 36: just over a third of a second, which is long enough to read as
/// something arriving and settling, and short enough that the user is not held
/// at a blank surface for the rest of the presentation window. Everything on this
/// surface -- the body's radius, the sweep's orbit, the wordmark's opening --
/// reads this one number, so the whole screen is one gesture.
const IGNITE_TICKS: u32 = 36;

// -----------------------------------------------------------------------------
// The sweep
// -----------------------------------------------------------------------------

/// Ticks per step of the sweep: a full turn every 64 ticks, 0.64s at the
/// kernel's 10ms tick. Unhurried enough to read as a body at work rather than as
/// a strobing light, and it never stops, because nothing here has measured when
/// it could.
const SWEEP_STEP_TICKS: u32 = 2;

/// The sweep's span, in 32nds of a turn: 24 of 32, three quarters of the circle.
/// The remaining eighth is the gap the head is travelling into, and it is what
/// makes the sweep read as travelling rather than as a ring that is merely lit.
const SWEEP_SPAN: usize = 24;

/// The bright leading edge, in the same 32nds: two steps, 22.5 degrees, which is
/// the head of the comet. WHY the head is brighter than the tail: without it the
/// sweep reads as a static "C" that happens to rotate, and with it the direction
/// of travel is legible at Device Studio's presentation scale.
const SWEEP_HEAD: usize = 2;

/// The tail's ink, as a share of the full accent in 256ths, mixed onto the
/// track's own colour so the tail and the track are two points on one ramp.
const TAIL_MIX: u32 = 132;

// -----------------------------------------------------------------------------
// The wordmark
// -----------------------------------------------------------------------------

/// The brand name, set as outline type.
///
/// WHY the outline face and not the magnified bitmap cell: the bitmap face is a
/// 16x24 cell rendered at an integer multiple, so this wordmark at rung 3 would
/// be a 48px cell with a 51px cap -- a magnified edge, which reads as a soft ramp
/// rather than as an edge. `design::typeface` rasterises real outlines at any em,
/// so the stroke lands where the outline puts it. This is the shell's largest
/// run and the natural place for it.
const WORDMARK: &[u8] = b"Inter OS";

/// Rung of the wordmark, in the shell's own scale.
///
/// WHY 3 and no larger: it is the biggest run the rasteriser will accept. The
/// coverage buffer is bounded at 256px wide and 96 rows tall, and the face's
/// line box is 1.318 em tall, so a rung above this one is refused outright and
/// the wordmark would not be drawn at all. At rung 3 the run measures 223x94 and
/// is the largest single-line wordmark this surface can set.
const WORDMARK_SCALE: usize = 3;

/// Advance of `WORDMARK` at the em [`outline_text::em_for_scale`] gives rung 3,
/// measured with `typeface::text_advance_px(b"Inter OS", 72)` = 223.
///
/// WHY measured rather than derived here: the left edge has to be a constant,
/// and a width guessed from a cell would put the run off-centre by however much
/// the guess was wrong. Recompute this if the rung or the word changes.
const WORDMARK_W: usize = 223;

/// Left edge of the run, centred on the surface.
const WORDMARK_X: usize = (W - WORDMARK_W) / 2;

/// Height of the wordmark's line box: the face's ascent plus its descent at the
/// 72px em, 75 + 19.
const WORDMARK_LINE_H: usize = 94;

/// Everything this surface stacks: the mark's group, one spacing step, the name.
const STACK_H: usize = 2 * SWEEP_R + spacing::XXL + WORDMARK_LINE_H;

/// Top of the wordmark's line box.
///
/// The stack, top to bottom: the mark's group is `2 * SWEEP_R` = 116 rows, one
/// `spacing::XXL` below it, then the wordmark's own 94-row line box. That is 258
/// of the 350 rows `main.rs` repaints, so 46 rows fall above the group and below
/// the name, and the stack is centred in the band rather than resting on its
/// floor. The rows and the height are the band's own, so the derivation moves
/// with the call site instead of drifting away from it.
const WORDMARK_TOP: usize = BAND_TOP + (BAND_ROWS - STACK_H) / 2 + 2 * SWEEP_R + spacing::XXL;

/// Baseline of the outline run: the face's own ascent, 75 rows above its line
/// box's top.
///
/// WHY the face's ascent and not a bitmap cell's offset: the two faces do not
/// agree on where the baseline is, and a baseline carried over from the cell
/// puts the outline's ink out of its own gate.
const WORDMARK_BASELINE: usize = WORDMARK_TOP + 75;

// -----------------------------------------------------------------------------
// The painter
// -----------------------------------------------------------------------------

/// The colour of one pixel of the boot presentation.
///
/// Everything the animation can know arrives in `state`: `boot_frame` is the one
/// clock, advanced once per timer tick while this view is up, and `boot_progress`
/// is deliberately not read (see the module note). The painter is a pure
/// function of that one counter, so the same frame drawn twice gives the same
/// pixels and a capture is reproducible.
pub fn get_boot_screen_color(x: usize, y: usize, state: &LauncherState) -> u32 {
    let dx = x as i32 - BRAND_CX as i32;
    let dy = y as i32 - BRAND_CY as i32;
    // Shared by the body and the sweep, so the mark and the light around it
    // cannot disagree about which pixel is where.
    let distance_sq = (dx * dx + dy * dy) as u32;
    // One curve for the whole surface: the body ignites, the sweep takes up its
    // orbit, and the name opens, all from this number.
    let lit = ignite(state.boot_frame);

    // Drawn in the order they read, and each one is a box test away from the
    // next, so a pixel that belongs to none of them costs four comparisons and
    // a subtraction.
    if let Some(color) = brand_pixel(dx, dy, distance_sq, lit, state.boot_frame) {
        return color;
    }
    if let Some(color) = sweep_pixel(dx, dy, distance_sq, lit, state.boot_frame) {
        return color;
    }
    if let Some(color) = wordmark_pixel(x, y, lit) {
        return color;
    }
    THEME.bg
}

/// How far the one reveal has come, in 1024ths.
///
/// WHY eased out: a linear radius reads as a machine inflating, and a radius
/// that arrives quickly and then settles reads as something coming alive. The
/// curve is `1 - (1 - t)^2` in integers, so it costs one multiply.
#[inline]
fn ignite(frame: u32) -> i32 {
    let linear = frame.min(IGNITE_TICKS) as i32 * 1024 / IGNITE_TICKS as i32;
    let rest = 1024 - linear;
    1024 - rest * rest / 1024
}

/// Mixes two of this theme's colours, `t` in 0..=256, through the theme's own
/// blend so this file never assembles a colour by hand.
#[inline]
fn mix(a: u32, b: u32, t: u32) -> u32 {
    Theme::blend(a, b, t.min(256))
}

/// The body's radius for this frame: the reveal's own curve, so the mark grows
/// out of a point and settles rather than appearing.
#[inline]
fn body_radius(lit: i32) -> i32 {
    BODY_R as i32 * lit / 1024
}

/// The mark: the body, and the pool of its own light where it meets the surface.
///
/// WHY this is the System Space orb's shading rather than a blue disc: the brand
/// mark has to be recognisably the body that sphere grows into, or the OS
/// carries two unrelated orbs. Every term is the one that screen uses -- the
/// chord height of a sphere, a light rotated in the plane of the surface, a
/// diffuse ramp, a rim where the body turns away, and a specular highlight
/// offset up and to the left of the light vector. Only the size and the backdrop
/// differ, and the backdrop is why one term changes its sign (see the glow).
fn brand_pixel(dx: i32, dy: i32, distance_sq: u32, lit: i32, frame: u32) -> Option<u32> {
    let r = body_radius(lit);
    let r2 = (r * r) as u32;

    if distance_sq <= r2 {
        if r < 1 {
            // The ignition point: one pixel before the body has a size, so there
            // is no normal to shade and nowhere to put a highlight. It is the
            // only pixel the mark is at frame 0, and it is what makes the first
            // presented frame a lit surface rather than a blank one.
            return Some(THEME.accent);
        }
        // Height of the sphere above this pixel, from the same chord geometry the
        // System Space orb uses. The walk is bounded by the radius -- 34 steps
        // at the resting size -- so it is constant time for this body, which is
        // what lets the per-pixel path afford a real sphere at all.
        let z2 = r2 - distance_sq;
        let mut z = 0i32;
        while (z + 1) * (z + 1) <= z2 as i32 {
            z += 1;
        }
        // The light, rotated to this frame's turn: the same resting angle and the
        // same in-plane rotation, so the highlight sweeps this body exactly as it
        // sweeps the System Space sphere.
        let (lx, ly, lz) = mark_light(frame);
        let dot = (lx * dx + ly * dy + lz * z) / (r * 4);
        let diffuse = (dot / 3).clamp(0, 6);
        // The body: `bg` toward `accent`, lifted by the diffuse ramp. WHY the
        // ramp is spread wider than that screen's own step of 8: this body is 34
        // rows where that one is 52, and the integer ramp settles on 0..2 here
        // rather than 0..6, so a step the same size would leave the whole body
        // inside its bottom two steps. The direction of the ramp is that
        // screen's; only how far it reaches is this mark's.
        let mut color = mix(THEME.bg, THEME.accent, 40 + diffuse as u32 * 70);
        // A rim that reads as the far side of the body, brightening where the
        // surface turns away from the viewer.
        if z <= 12 {
            color = mix(color, THEME.accent, ((12 - z) * 9) as u32);
        }
        // The specular highlight, offset up and to the left of the light vector so
        // it lands on the lit side rather than on the vector itself.
        //
        // WHY the offset divides the gain back out: that screen's highlight sits
        // beside its body's centre because its lamp is one unit there, and the
        // offset is a share of the lamp. At this lamp's magnitude the same share
        // would carry the highlight outside a 34-row body entirely.
        let spec_dx = dx + (lx / LAMP_GAIN) * 5 / 4;
        let spec_dy = dy + (ly / LAMP_GAIN) * 5 / 4;
        let spec_d2 = (spec_dx * spec_dx + spec_dy * spec_dy) as u32;
        let spec_r2 = ((r * r) / 36).max(1) as u32;
        if spec_d2 <= spec_r2 {
            let intensity = spec_r2 - spec_d2;
            color = mix(color, THEME.text, intensity * 90 / spec_r2);
        }
        // A hot core inside the highlight, so the lit side reads as a specular
        // point rather than as another gradient.
        if spec_d2 <= 25 {
            color = mix(color, THEME.text, (25 - spec_d2) * 60 / 25);
        }
        return Some(color);
    }

    // The pool of the body's own light where it meets the surface.
    //
    // WHY a glow and not a drop shadow: that orb casts one onto a near-white
    // page, and this surface is `bg` -- there is nothing darker than the
    // backdrop to cast into. The ellipse the System Space orb's shadow occupies
    // is kept exactly -- same offset below the body, same 60-by-52 squash, same
    // falloff -- and holds the accent instead of a shade, which is what a lit
    // sphere sitting on a dark surface does anyway.
    //
    // WHY the bounding box is tested before the ellipse: the ellipse is two
    // divisions, and this painter is asked about every pixel of the band. The
    // box is four comparisons, and it rejects all but the ellipse's own 78x11
    // rows, so the divisions run roughly a thousand times a frame instead of a
    // quarter of a million.
    let below = dy - (r * 56 / 52);
    let wide = (r * 60 / 52).max(1);
    let tall = (r * 10 / 52).max(1);
    if dx >= -wide && dx <= wide && below >= -tall && below <= tall {
        let reach = dx * dx * 100 / (wide * wide) + below * below * 100 / (tall * tall);
        if reach <= 100 {
            return Some(mix(
                THEME.bg,
                THEME.accent,
                ((100 - reach) * 60 / 100) as u32,
            ));
        }
    }
    None
}

/// The mark's light direction for this frame, scaled by [`LAMP_GAIN`].
///
/// WHY the same rotation the System Space orb's uses: the highlight is the only
/// thing on a shaded sphere that moves, so sharing its rotation is what makes
/// the two bodies recognisably the same body. The rotation is in the plane of
/// the surface -- the light's (x, y) turn and its z does not -- which is what
/// sweeps the highlight around the body instead of changing its shading.
fn mark_light(frame: u32) -> (i32, i32, i32) {
    // One step every six ticks: a full turn every 192 ticks, a little under two
    // seconds, unhurried enough to read as a body turning rather than as a light
    // strobing.
    let (sin, neg_cos) = TURN[((frame / 6) % TURN_STEPS as u32) as usize];
    let cos = -neg_cos;
    let gain = LAMP_GAIN;
    (
        (LIGHT_X * cos - LIGHT_Y * sin) / TRIG_SCALE * gain,
        (LIGHT_X * sin + LIGHT_Y * cos) / TRIG_SCALE * gain,
        LIGHT_Z * gain,
    )
}

/// The progress indication: an indeterminate sweep circling the mark.
///
/// WHY it is indeterminate, and why that is stated by its behaviour rather than
/// by a label: `boot_progress` is a count of timer ticks against the 180-tick
/// window `main.rs` hands this view, so it measures elapsed time and nothing
/// else. Nothing on this surface has reported a completed step, so a bar filled
/// from that counter would claim a fraction of work that was never measured, and
/// a percentage would claim a rate nobody has observed. This sweep instead turns
/// at a constant rate and never accumulates: it says the device is working, and
/// it says nothing about how much work is left, because nothing knows.
fn sweep_pixel(dx: i32, dy: i32, distance_sq: u32, lit: i32, frame: u32) -> Option<u32> {
    let r = SWEEP_R as i32 * lit / 1024;
    // Only pixels whose radius falls inside the 4-row stroke can be part of the
    // sweep, and that is a comparison rather than the eight multiplications the
    // wedge tests cost. It is also what keeps the direction table off the path
    // for the rest of the band.
    let inner = (r - SWEEP_STROKE).max(0);
    if distance_sq < (inner * inner) as u32
        || distance_sq > ((r + SWEEP_STROKE) * (r + SWEEP_STROKE)) as u32
    {
        return None;
    }

    let head = ((frame / SWEEP_STEP_TICKS) % TURN_STEPS as u32) as usize;
    // The gap the head is travelling into: the eighth of the turn ahead of it.
    let gap_from = TURN[(head + 1) % TURN_STEPS];
    let gap_to = TURN[(head + TURN_STEPS - SWEEP_SPAN) % TURN_STEPS];
    if within(dx, dy, gap_from, gap_to) {
        return Some(THEME.border);
    }
    // The head: the last steps of the tail, at the full accent.
    let head_from = TURN[(head + TURN_STEPS - SWEEP_HEAD) % TURN_STEPS];
    let head_to = TURN[head % TURN_STEPS];
    if within(dx, dy, head_from, head_to) {
        return Some(THEME.accent);
    }
    // The rest of the tail, one point on the ramp the track is the other end of.
    Some(mix(THEME.border, THEME.accent, TAIL_MIX))
}

/// Whether the direction of (`dx`, `dy`) lies inside the wedge swept clockwise
/// from `from` to `to`.
///
/// WHY cross products and not an angle: both boundary rays are directions in
/// 64ths, and the sign of a cross product says which side of a ray a vector
/// falls on. Two multiplications decide membership where an atan2 would cost a
/// division and a quadrant dispatch on every pixel of the ring -- and this only
/// ever has to be right about which side of two lines a pixel is on. The arcs'
/// edges are radial either way, so the 32-step turn table introduces no visible
/// banding here: the sweep has no gradient for it to quantise.
#[inline]
fn within(dx: i32, dy: i32, from: (i32, i32), to: (i32, i32)) -> bool {
    cross(from.0, from.1, dx, dy) >= 0 && cross(dx, dy, to.0, to.1) >= 0
}

/// The z of the cross product, whose sign says whether `(bx, by)` is clockwise
/// from `(ax, ay)` on this surface (where y grows downward).
#[inline]
fn cross(ax: i32, ay: i32, bx: i32, by: i32) -> i32 {
    ax * by - ay * bx
}

/// The wordmark: the name, opening out of the same reveal the mark ignites in.
fn wordmark_pixel(x: usize, y: usize, lit: i32) -> Option<u32> {
    let mark = wordmark_frame();
    let run = mark.run.as_ref()?;
    // Gate on the placed run's own ink box, which is the window the run reports
    // about itself rather than a cell measured for the bitmap face.
    //
    // WHY that matters more here than anywhere else in the shell: this is the
    // largest type the shell sets, the outline's ascent reaches 75 rows above
    // its baseline and its descent 19 below, and a cell-shaped window would
    // silently clip the tall glyphs and nothing else would notice.
    if x < mark.box_x || x - mark.box_x >= mark.box_w {
        return None;
    }
    if y < mark.box_y || y - mark.box_y >= mark.box_h {
        return None;
    }
    // The reveal: a clip that opens from the centre of the word outward. It is a
    // clip rather than a fade because a fade would make the name look as though
    // it were already there and merely dim, and the first boot is the one moment
    // the device has to introduce itself. It saturates, so it happens once.
    let half = (mark.box_w as i32 + 1) / 2;
    if (x as i32 - (W / 2) as i32).abs() > half * lit / 1024 {
        return None;
    }
    run.pixel(x, y, THEME.text, THEME.bg)
}

/// The placed wordmark and the gate its own ink box provides.
struct Wordmark {
    /// Whether placement has been attempted. A refusal is remembered rather than
    /// retried per pixel, and it leaves the wordmark undrawn.
    resolved: bool,
    run: Option<OutlineRun>,
    box_x: usize,
    box_y: usize,
    box_w: usize,
    box_h: usize,
}

/// The wordmark, reachable from a `static`.
///
/// Why `Sync` is asserted rather than derived: `UnsafeCell` is deliberately not
/// `Sync`, and this is mutable shared state, so something has to say the
/// accesses are serialised. They are: `get_boot_screen_color` has exactly one
/// caller -- `render_band`, in the launcher task -- the guest runs no second
/// thread, nothing in this file calls back into the painter, and the borrow
/// [`wordmark_frame`] takes does not outlive the pixel it was taken for.
/// Asserted here, in the one place that needs it.
struct WordmarkMemo(UnsafeCell<Wordmark>);

// SAFETY: see `WordmarkMemo`'s own note. The mutable access is confined to the
// borrow in `wordmark_frame`, which nothing else in this file can reach while it
// is live.
unsafe impl Sync for WordmarkMemo {}

static WORDMARK_RUN: WordmarkMemo = WordmarkMemo(UnsafeCell::new(Wordmark {
    resolved: false,
    run: None,
    box_x: 0,
    box_y: 0,
    box_w: 0,
    box_h: 0,
}));

/// The wordmark, placed once for the life of this surface.
///
/// WHY once and not per pixel: `OutlineRun::place` walks the advance table on
/// every call and rasterises on a miss, so asking it once per pixel of a 223x94
/// line box would cost more than the rest of this band put together. The run's
/// key -- its origin, its em and its bytes -- never changes while this view is
/// up, and the reveal is a clip applied after the box test rather than a second
/// placement, so one placement answers every frame and the outline coverage is
/// rasterised exactly once per boot.
fn wordmark_frame() -> &'static Wordmark {
    // SAFETY: the single mutable access to the memo, taken here and released
    // when this function returns. Nothing reached from here re-enters it.
    let slot = unsafe { &mut *WORDMARK_RUN.0.get() };
    if !slot.resolved {
        slot.resolved = true;
        // The em comes from the shell's own derivation, so this run is the same
        // size as the same rung set on any other screen.
        let em = outline_text::em_for_scale(WORDMARK_SCALE);
        if let Some(run) = OutlineRun::place(WORDMARK_X, WORDMARK_BASELINE, em, WORDMARK) {
            let (bx, by, bw, bh) = run.bounds();
            slot.box_x = bx;
            slot.box_y = by;
            slot.box_w = bw;
            slot.box_h = bh;
            slot.run = Some(run);
        }
        // A refusal leaves the box empty and the name undrawn. It does not fall
        // back to the bitmap face: this screen's one run is the brand wordmark,
        // the two faces disagree about advance and about where the baseline is,
        // and a per-pixel fallback would draw the glyph from both at once -- an
        // outline under a hard-edged bitmap ghost one pixel off.
    }
    slot
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The progress indication must not read `boot_progress`.
    ///
    /// WHY this pin exists: that counter is a count of timer ticks against the
    /// 180-tick window `main.rs` hands this view, so it measures elapsed time and
    /// not work. A renderer that keys off it draws a number claiming completion
    /// that was never measured. This asserts the pixels do not move when the
    /// counter does.
    #[test]
    fn boot_progress_does_not_move_the_progress_indication() {
        let mut state = LauncherState::new();
        state.boot_frame = 64;
        let (x, y) = (W / 2 + SWEEP_R, H / 2);
        state.boot_progress = 0;
        let before = get_boot_screen_color(x, y, &state);
        state.boot_progress = 1000;
        let after = get_boot_screen_color(x, y, &state);
        assert_eq!(before, after, "the sweep must not read boot_progress");
        assert_ne!(before, THEME.bg, "the sweep has to be visibly lit");
    }

    /// The sweep travels: the same pixel is the gap at one frame and the tail at
    /// another.
    ///
    /// WHY this pin exists alongside the honesty one: a frozen sweep is the
    /// defect that made this screen look hung, and a progress-driven sweep would
    /// pass the honesty test while still freezing whenever the counter steps.
    /// Straight right of the mark, at step 8 of the turn: at frame 24 the head is
    /// at step 12 and that direction is in the tail; by frame 64 the head has come
    /// round to step 0 and the same direction is the gap it is travelling into.
    #[test]
    fn the_sweep_travels_with_the_frame_counter() {
        let mut state = LauncherState::new();
        let (x, y) = (W / 2 + SWEEP_R, H / 2);
        state.boot_frame = 24;
        let tail = get_boot_screen_color(x, y, &state);
        state.boot_frame = 64;
        let gap = get_boot_screen_color(x, y, &state);
        assert_eq!(gap, THEME.border, "the gap ahead of the head is the track");
        assert_ne!(tail, gap);
        assert_ne!(tail, THEME.bg, "the tail has to be visibly lit");
    }

    /// The mark ignites out of a point and settles: a pixel that is backdrop
    /// before the reveal has run is inside the body afterwards.
    #[test]
    fn the_mark_ignites_from_a_point_and_settles() {
        let mut state = LauncherState::new();
        let (x, y) = (W / 2, BRAND_CY - BODY_R + 6);
        state.boot_frame = 0;
        let first = get_boot_screen_color(x, y, &state);
        state.boot_frame = IGNITE_TICKS;
        let settled = get_boot_screen_color(x, y, &state);
        assert_eq!(first, THEME.bg, "the body has no size before the reveal");
        assert_ne!(
            settled, THEME.bg,
            "the same pixel is inside the body after it"
        );
    }

    /// The wordmark arrives once and then holds: nothing is drawn before the
    /// reveal reaches it, something is after, and a later frame does not re-run
    /// the reveal.
    #[test]
    fn the_wordmark_reveals_once_and_then_holds() {
        let mut state = LauncherState::new();
        state.boot_frame = 0;
        let hidden = inked_in_wordmark(&state);
        state.boot_frame = IGNITE_TICKS;
        let shown = inked_in_wordmark(&state);
        state.boot_frame = IGNITE_TICKS * 2;
        let later = inked_in_wordmark(&state);
        assert_eq!(hidden, 0, "no part of the name is there before the reveal");
        assert!(shown > 0, "the name has to arrive");
        assert_eq!(shown, later, "the reveal must not re-run");
    }

    /// The painter stays a bounded walk: a corner pixel of the surface, which
    /// belongs to no element, is the backdrop and nothing else.
    #[test]
    fn the_surface_stays_backdrop_outside_the_stack() {
        let state = LauncherState::new();
        assert_eq!(get_boot_screen_color(0, 0, &state), THEME.bg);
        assert_eq!(get_boot_screen_color(W - 1, H - 1, &state), THEME.bg);
    }

    /// Nothing this surface draws reaches outside the rows `main.rs` repaints.
    ///
    /// WHY this pin exists: the view is driven by `render_band(surface, state,
    /// H / 2 - 130, H / 2 + 220)`, so anything the stack places above or below
    /// that band is never repainted and would show as a ghost of a previous
    /// surface. The two constants above are the band's own, so this test fails if
    /// the stack ever outgrows the rows it is given.
    #[test]
    fn the_stack_stays_inside_the_rows_main_rs_repaints() {
        assert!(
            BRAND_CY >= BAND_TOP + SWEEP_R,
            "the mark's sweep runs off the top of the band"
        );
        assert!(
            WORDMARK_TOP + WORDMARK_LINE_H <= BAND_TOP + BAND_ROWS,
            "the name's line box runs off the bottom of the band"
        );
    }

    /// Inked pixels inside the wordmark's own box, for the reveal test.
    fn inked_in_wordmark(state: &LauncherState) -> usize {
        let mark = wordmark_frame();
        let mut ink = 0usize;
        for y in mark.box_y..mark.box_y + mark.box_h {
            for x in mark.box_x..mark.box_x + mark.box_w {
                if get_boot_screen_color(x, y, state) != THEME.bg {
                    ink += 1;
                }
            }
        }
        ink
    }
}
