// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The platform's type ladder, and the measurements that fix it.
//!
//! WHY this module exists: the design document writes its type scale in the
//! sizes a reader sees, and there were two readings of those numbers -- as
//! font sizes and as cap heights. They differ by a third, and getting it
//! wrong made every screen's text smaller than the document asks for.
//!
//! Measuring the reference screens settles the reading. On the reference
//! calculator -- a 720-pixel-wide screen -- the status clock stands thirteen
//! pixels tall, a key label fifteen, the title twenty-three and the result
//! thirty-one. Those are cap heights, so the ladder names cap heights and the
//! em follows from one.
//!
//! The measured numbers are a floor, not a ceiling: they say what the smallest
//! legible run on a panel of this size looks like. What the rungs are drawn
//! *at* is settled against the panels this device is compared with, and a type
//! scale only means something relative to the width it is read on -- see
//! [`crate::font::cap_for_scale`], which owns the values. This module is where
//! the bands each rung is allowed to sit in are written down once, and
//! `tests/test_type_scale_contract.py` reads them from here rather than keeping
//! its own copy.

#[cfg(test)]
mod cap_ladder_tests {
    use crate::design::outline_text;
    use crate::design::typography;

    /// The band each rung is allowed to sit in.
    ///
    /// WHY these are the bands and not the document's original ranges: the
    /// document's caption of twelve to fourteen and body of fifteen to eighteen
    /// were written for a page, and on a 720-pixel panel held at reading
    /// distance they come out under what the phones this device is compared
    /// with draw -- iOS sets its body at three per cent of the panel's width
    /// and Android's body-large at 2.77%, where the document's body rung was
    /// 2.50% and its caption 1.81%. Each rung was raised to the share those
    /// systems use, and the band below is the raised role, so the ladder cannot
    /// drift back to a size a person on this panel cannot read.
    ///
    /// The upper rungs are unchanged: a heading and a display were already
    /// large enough for the panel.
    pub const LADDER_BANDS: [(u32, u32, u32, u32); 6] = [
        (1, typography::CAPTION_CAP_PX, 15, 16),
        (2, typography::BODY_CAP_PX, 19, 21),
        (3, typography::SECTION_CAP_PX, 25, 27),
        (4, typography::TITLE_CAP_PX, 33, 35),
        (5, typography::HEADING_CAP_PX, 36, 44),
        (6, typography::DISPLAY_CAP_PX, 48, 56),
    ];

    /// Every rung sits inside the band written for it.
    #[test]
    fn every_rung_is_inside_the_band_written_for_it() {
        for (rung, cap, low, high) in LADDER_BANDS {
            assert!(
                cap >= low && cap <= high,
                "rung {rung} draws a {cap}px cap, outside its {low}..={high} band"
            );
        }
    }

    /// WHAT THE BANDS CONSTRAIN, and what the person's text-size setting does.
    ///
    /// The bands above are the platform's ladder: the number a rung draws at
    /// when nobody has asked for anything else. A published text-size level is
    /// not a new rung on the ladder -- it is the same rung multiplied by the
    /// setting's percentage, and it therefore lands outside these bands by
    /// design. This test writes that down as a number rather than as a comment,
    /// because the honest reading of "the display rung may not exceed 56px" and
    /// "extra large draws the display rung at 84px" look like a contradiction
    /// until someone checks which of the two the band governs.
    ///
    /// WHY the largest setting is not clamped back inside the band: a band is a
    /// floor and a ceiling for a *default*, and a person who asks for 150%
    /// asked to leave it. What bounds the top instead is the legibility floor
    /// below (`scale_cap_height` never draws a cap under ten pixels at
    /// compact) and the fact that a surface only scales where its own boxes are
    /// re-derived from the same rung -- a box that cannot grow is a box that
    /// does not take the setting, and says so.
    #[test]
    fn the_bands_govern_the_ladder_and_the_published_rung_sits_above_them() {
        // The display rung's band is 48..=56; extra large draws it at 84.
        assert!(crate::font::cap_for_user_scale(6, 3) > 56);
        // And compact draws it below the band's own floor.
        assert!(crate::font::cap_for_user_scale(6, 0) < 48);
        // Normal is the ladder: every rung inside its band at the default level.
        for (rung, cap, low, high) in LADDER_BANDS {
            let at_normal = crate::font::cap_for_user_scale(rung as usize, 1);
            assert_eq!(at_normal, cap, "the default level must not move a rung");
            assert!(at_normal >= low && at_normal <= high);
        }
        // The legibility floor holds at every rung the ladder has, compact
        // included -- the one bound the setting does respect.
        for rung in 1..=7usize {
            assert!(crate::font::cap_for_user_scale(rung, 0) >= 10);
            assert!(crate::font::cap_for_user_scale(rung, 3)
                >= crate::font::cap_for_scale(rung));
        }
    }

    /// The sizes measured off the reference screens are all covered.
    ///
    /// WHY these are here and not only in a comment: the reading of the
    /// document's numbers is what the whole suite's typography rests on, and
    /// a reading that is wrong is invisible until every screen looks small.
    #[test]
    fn the_ladder_covers_the_sizes_measured_on_the_reference_screens() {
        // The reference calculator, measured on its seven-hundred and twenty
        // pixel wide screen: a status clock thirteen pixels tall, a key label
        // fifteen, the title twenty-three and the result thirty-one.
        let measured = [
            (typography::CAPTION_CAP_PX, 13, "status clock"),
            (typography::BODY_CAP_PX, 15, "key label"),
            (typography::SECTION_CAP_PX, 23, "title"),
            (typography::TITLE_CAP_PX, 31, "result"),
        ];
        for (rung, size, role) in measured {
            assert!(
                rung >= size,
                "a {role} measured {size}px tall on the reference screen is drawn at {rung}px here"
            );
        }
        // The smallest thing on the ladder is not smaller than the smallest
        // thing measured on a screen a person could read: the reference
        // calculator's key label, at fifteen pixels.
        assert!(typography::CAPTION_CAP_PX >= 15);
    }

    #[test]
    fn the_ladder_is_monotone() {
        let ladder = [
            typography::CAPTION_CAP_PX,
            typography::BODY_CAP_PX,
            typography::SECTION_CAP_PX,
            typography::TITLE_CAP_PX,
            typography::HEADING_CAP_PX,
            typography::DISPLAY_CAP_PX,
        ];
        for pair in ladder.windows(2) {
            assert!(
                pair[1] > pair[0],
                "the ladder is not increasing: {pair:?}"
            );
        }
    }

    /// Roles must be distinguishable, or a screen of them reads as one size.
    ///
    /// WHY a floor on the gap: the complaint this guards against was a flat
    /// hierarchy -- a card title and a caption within a pixel of each other,
    /// so nothing on the screen looked like a heading. Ratio alone is not
    /// enough at the small end (13 to 15 is a 1.15x step and is invisible), so
    /// the two smallest steps are also pinned in pixels.
    #[test]
    fn neighbouring_rungs_are_separated() {
        for (rung, cap, _, _) in LADDER_BANDS {
            let next = crate::font::cap_for_scale((rung + 1) as usize);
            assert!(
                next >= cap + 4 && next * 100 >= cap * 113,
                "rung {} at {}px and rung {} at {}px read as one size",
                rung, cap, rung + 1, next
            );
        }
    }

    #[test]
    fn a_cap_height_grows_the_em_with_it() {
        let small = crate::font::em_for_cap_scale(typography::CAPTION_CAP_PX);
        let large = crate::font::em_for_cap_scale(typography::DISPLAY_CAP_PX);
        assert!(small > 0);
        assert!(large > small * 2, "the display em should be several captions");
    }

    #[test]
    fn a_run_can_be_placed_at_a_cap_height() {
        let run = outline_text::place_run_cap(0, 0, typography::BODY_CAP_PX, b"InterOS", None, false)
            .expect("a run should place at a body cap height");
        // The run's ink is what a caller draws, so a run that inks nothing is
        // a run that cannot be shown.
        let mut inked = false;
        'outer: for y in 0..64 {
            for x in 0..200 {
                if run.pixel(x, y, 0xFF000000, 0xFFFFFFFF).is_some() {
                    inked = true;
                    break 'outer;
                }
            }
        }
        assert!(inked, "a placed run should ink something");
    }
}
