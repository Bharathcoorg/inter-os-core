//! Minimal Ring-3 Guest Application Proving the `DrawTarget` Surface Adapter
//!
//! Validates:
//! - `#![no_std]`, `#![no_main]` userspace application execution in Ring 3
//! - Integration with `interos_std::design::draw_target::{Surface, DrawTarget, ...}`
//! - Solid card drawing via `DrawTarget::fill_solid`
//! - Background clearing via `DrawTarget::clear`
//! - Title and badge typography via `DrawTarget::draw_iter`
//! - Contiguous chunk writes to KMS Client Surface and presentation flip
//! - Clean lifecycle supervision and interactive event handling

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use interos_std as std;
use std::design::draw_target::{DrawTarget, Pixel, Point, Rectangle, Size, Surface, SurfaceColor};

const W: usize = 720;
const H: usize = 1440;
const BAND_H: usize = 240;
const NUM_BANDS: usize = H / BAND_H; // 6 bands = 1440 rows

// 720 * 240 pixels * 4 bytes = 691,200 bytes (~675 KB in BSS).
// Well within the 3.75 MB user space ceiling below USER_STACK_BOTTOM.
static mut BAND_BUF: [u32; W * BAND_H] = [0; W * BAND_H];

const SCREEN_BG: SurfaceColor = SurfaceColor::from_rgb(0x11, 0x12, 0x14);
const CARD_BG: SurfaceColor = SurfaceColor::from_rgb(0x1F, 0x23, 0x28);
const CARD_BORDER: SurfaceColor = SurfaceColor::from_rgb(0x35, 0x38, 0x3D);
const ACCENT_BLUE: SurfaceColor = SurfaceColor::from_rgb(0x25, 0x63, 0xEB);
const BADGE_BG: SurfaceColor = SurfaceColor::from_rgb(0x16, 0x18, 0x1A);

fn draw_text_target<T: DrawTarget<Color = SurfaceColor>>(
    target: &mut T,
    band_y0: i32,
    x0: usize,
    y0: usize,
    scale: usize,
    text: &[u8],
    color: u32,
    bold: bool,
) {
    let total_w = std::font::measure_bold(text, scale);
    let total_h = std::font::line_height(scale);
    let mut pixels = [Pixel(Point::zero(), SurfaceColor(0)); 256];
    let mut count = 0;

    for dy in 0..total_h {
        let global_y = y0 + dy;
        let local_y = (global_y as i32) - band_y0;
        for dx in 0..total_w {
            let global_x = x0 + dx;
            if let Some(c) = std::font::test_text_aa(global_x, global_y, x0, y0, scale, text, color, 0, bold) {
                pixels[count] = Pixel(
                    Point::new(global_x as i32, local_y),
                    SurfaceColor(c),
                );
                count += 1;
                if count == pixels.len() {
                    let _ = target.draw_iter(pixels[..count].iter().copied());
                    count = 0;
                }
            }
        }
    }
    if count > 0 {
        let _ = target.draw_iter(pixels[..count].iter().copied());
    }
}

fn render_frame(surface_id: u32) {
    let card_x: i32 = 48;
    let card_y: i32 = 260;
    let card_w: u32 = 624;
    let card_h: u32 = 560;

    let inner_x: i32 = 80;
    let inner_y: i32 = 500;
    let inner_w: u32 = 560;
    let inner_h: u32 = 180;

    for band in 0..NUM_BANDS {
        let band_y0 = (band * BAND_H) as i32;
        let buf = unsafe { &mut *core::ptr::addr_of_mut!(BAND_BUF) };

        let mut target = match Surface::new(buf, W, BAND_H) {
            Ok(s) => s,
            Err(_) => return,
        };

        // 1. Clear background via DrawTarget::clear
        let _ = target.clear(SCREEN_BG);

        // 2. Card background via DrawTarget::fill_solid
        let rel_card_y = card_y - band_y0;
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(card_x, rel_card_y), Size::new(card_w, card_h)),
            CARD_BG,
        );

        // 3. Card accent banner (8px top bar)
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(card_x, rel_card_y), Size::new(card_w, 8)),
            ACCENT_BLUE,
        );

        // 4. Card borders (top, bottom, left, right 2px)
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(card_x, rel_card_y), Size::new(card_w, 2)),
            CARD_BORDER,
        );
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(card_x, rel_card_y + (card_h as i32) - 2), Size::new(card_w, 2)),
            CARD_BORDER,
        );
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(card_x, rel_card_y), Size::new(2, card_h)),
            CARD_BORDER,
        );
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(card_x + (card_w as i32) - 2, rel_card_y), Size::new(2, card_h)),
            CARD_BORDER,
        );

        // 5. Inner spec card via DrawTarget::fill_solid
        let rel_inner_y = inner_y - band_y0;
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(inner_x, rel_inner_y), Size::new(inner_w, inner_h)),
            BADGE_BG,
        );
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(inner_x, rel_inner_y), Size::new(inner_w, 1)),
            CARD_BORDER,
        );
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(inner_x, rel_inner_y + (inner_h as i32) - 1), Size::new(inner_w, 1)),
            CARD_BORDER,
        );

        // 6. Typography via DrawTarget::draw_iter
        // Card title
        draw_text_target(&mut target, band_y0, 80, 310, 3, b"DrawTarget Card", 0xFFF5_F6F7, true);
        // Subtitle
        draw_text_target(&mut target, band_y0, 80, 400, 2, b"Standard Surface Adapter", 0xFF9A_A0A6, false);
        // Feature tags inside inner badge
        draw_text_target(&mut target, band_y0, 108, 530, 2, b"Zero Allocations Verified", 0xFF2E_8B57, true);
        draw_text_target(&mut target, band_y0, 108, 580, 2, b"ET_EXEC Static Relocation", 0xFF25_63EB, false);
        draw_text_target(&mut target, band_y0, 108, 630, 2, b"Span Fills & Row Bounds", 0xFFF5_F6F7, false);

        // 7. Navigation pill at bottom of screen
        let rel_pill_y = 1418 - band_y0;
        let _ = target.fill_solid(
            &Rectangle::new(Point::new(260, rel_pill_y), Size::new(200, 6)),
            SurfaceColor::from_rgb(0xFF, 0xFF, 0xFF),
        );

        // 8. Write contiguous band to KMS Client Surface in bounded spans (<= 65536)
        let band_base = (band * BAND_H) * W;
        let mut written = 0;
        let total = W * BAND_H;
        while written < total {
            let chunk_size = (total - written).min(16384);
            let res = std::surface_write(
                5,
                surface_id,
                band_base + written,
                &buf[written..written + chunk_size],
            );
            if res.is_err() {
                std::log(0, "[CARD_DEMO] surface_write failed!").ok();
            }
            written += chunk_size;
        }
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    std::log(0, "[CARD_DEMO (Ring 3)] Starting DrawTarget proof application...").ok();

    // 1. Create KMS Client Surface (z=200)
    let surface = match std::surface_create(5, W as u32, H as u32, 200) {
        Ok(s) => {
            std::log(0, "[CARD_DEMO] Surface created (z=200). Rendering card frame...").ok();
            s
        }
        Err(_) => {
            std::log(0, "[CARD_DEMO] Failed to create surface.").ok();
            std::exit_with_code(1);
        }
    };

    // 2. Render initial frame using DrawTarget and flip
    render_frame(surface);
    let _ = std::surface_flip(5);
    std::log(0, "[CARD_DEMO] Initial frame presented via hardware scanout.").ok();

    // 3. Event Loop: wait for gesture or exit
    loop {
        if let Ok(_events) = std::event_poll(std::EVENT_TOUCH_INPUT, 100) {
            let mut input = std::InputEvent {
                kind: 0,
                x: 0,
                y: 0,
                dx: 0,
                dy: 0,
                scale_milli: 1000,
            };
            while std::input_event_poll(&mut input).is_ok() {
                // Navigation gestures (Back, Home, Recents, Fling)
                if input.kind == 4 || input.kind == 6 || input.kind == 7 || input.kind == 8 || input.kind == 10 {
                    std::log(0, "[CARD_DEMO] Navigation gesture received. Exiting.").ok();
                    let _ = std::surface_destroy(5, surface);
                    std::exit_with_code(0);
                }
                // Tap on bottom home gesture pill
                if (input.kind == 1 || input.kind == 2) && input.y >= 1400 {
                    std::log(0, "[CARD_DEMO] Home bar tapped. Exiting.").ok();
                    let _ = std::surface_destroy(5, surface);
                    std::exit_with_code(0);
                }
            }
        }
        std::yield_now();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    std::log(0, "[PANIC] card_demo encountered an error!").ok();
    std::exit_with_code(1);
}
