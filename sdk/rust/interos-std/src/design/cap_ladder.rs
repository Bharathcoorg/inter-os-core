//! The platform's type ladder, and the measurements that fix it.
//!
//! WHY this module exists: the design document writes its type scale in the
//! sizes a reader sees, and there were two readings of those numbers -- as
//! font sizes and as cap heights. They differ by a third, and getting it
//! wrong made every screen's text smaller than the document asks for.
//!
//! Measuring the reference screens settles it. On the reference calculator
//! -- a 720-pixel-wide screen -- the status clock stands thirteen pixels
//! tall, a key label fifteen, the title twenty-three and the result
//! thirty-one. The document's ranges are a caption of twelve to fourteen, a
//! body of fifteen to eighteen, a section of twenty to twenty-four and a
//! title of twenty-eight to thirty-two. Each measured size falls inside
//! the range for the role it plays, so the numbers are cap heights.
//!
//! The ladder therefore names cap heights, and the em follows from one.

#[cfg(test)]
mod cap_ladder_tests {
    use crate::design::outline_text;
    use crate::design::typography;

    /// The ladder, and the range the design document gives each rung.
    ///
    /// WHY the ranges are the document's and not the ones this test invented:
    /// the document is the specification, and the measured reference sizes
    /// are what check the implementation against it.
    #[test]
    fn every_rung_is_inside_the_range_the_document_gives() {
        let ladder = [
            (typography::CAPTION_CAP_PX, 12, 14),
            (typography::BODY_CAP_PX, 15, 18),
            (typography::SECTION_CAP_PX, 20, 24),
            (typography::TITLE_CAP_PX, 28, 32),
            (typography::HEADING_CAP_PX, 36, 44),
            (typography::DISPLAY_CAP_PX, 48, 56),
        ];
        for (cap, low, high) in ladder {
            assert!(
                cap >= low && cap <= high,
                "the cap height {cap} is outside the document's {low}..={high} range"
            );
        }
    }

    /// The sizes measured off the reference screens.
    ///
    /// WHY these are here and not only in a comment: the reading of the
    /// document's numbers is what the whole suite's typography rests on, and
    /// a reading that is wrong is invisible until every screen looks small.
    #[test]
    fn the_ladder_covers_the_sizes_measured_on_the_reference_screens() {
        // The reference calculator, measured on its seven-hundred and twenty
        // pixel wide screen: a status clock thirteen pixels tall, a key label
        // fifteen, the title twenty-three and the result thirty-one.
        assert_eq!(typography::CAPTION_CAP_PX, 13);
        assert!(typography::BODY_CAP_PX >= 15 && typography::BODY_CAP_PX <= 18);
        // The title measured twenty-two and two thirds, and the ladder's
        // section rung is the document's value for that role.
        assert!(
            typography::SECTION_CAP_PX >= 22 && typography::SECTION_CAP_PX <= 24,
            "the section rung is {} and the measured title was twenty-two and two thirds",
            typography::SECTION_CAP_PX
        );
        // The result measured thirty-one, which is the document's title band.
        assert!(
            typography::TITLE_CAP_PX >= 28 && typography::TITLE_CAP_PX <= 32,
            "the title rung is {} and the measured result was thirty-one",
            typography::TITLE_CAP_PX
        );
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
