
#[cfg(test)]
mod cap_ladder_tests {
    use crate::design::typography;
    use crate::design::outline_text;

    /// The design document's type scale, and the multiples the old ladder
    /// could reach.
    ///
    /// WHY this test exists: the old ladder was a set of multiples of the
    /// bitmap face's sixteen-pixel cell, so the sizes available were 16, 32,
    /// 48 and 96 pixels of cap height. The document asks for a caption of
    /// 12 to 14, a body of 15 to 18, a section heading of 20 to 24, a title
    /// of 28 to 32, a page heading of 36 to 44 and a display of 48 to 56 --
    /// and two of the six had no rung to sit on.
    #[test]
    fn the_cap_height_ladder_covers_the_design_document_sizes() {
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
                "the cap height {} is outside the document's {}..={} range",
                cap,
                low,
                high
            );
        }
    }

    #[test]
    fn a_cap_height_grows_the_em_with_it() {
        let small = outline_text::em_for_cap_height(typography::CAPTION_CAP_PX);
        let large = outline_text::em_for_cap_height(typography::DISPLAY_CAP_PX);
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
