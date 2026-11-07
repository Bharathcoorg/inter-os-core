// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! The search index: one list of everything the platform can be asked about, and
//! the ranking that decides what a query returns.
//!
//! WHY one index and not three searches: the user types one thing and expects
//! one answer. Three separate lists mean three separate verdicts, and the user
//! cannot tell which of them answered. So the shell builds one index --
//! installed packages, the platform's settings surfaces, and the files in the
//! shared storage -- and the query runs across all of it.
//!
//! WHY the index is built from the platform and not written out here: the
//! entries have to be things that exist. The apps come from the package
//! registry, the settings surfaces from the SDK's section catalogue (the same
//! one the Settings app renders), and the files from the VFS listing. A
//! hand-written list of app names would be a list of destinations that might
//! not be installed, and a hand-written list of settings would be a list of
//! sections the Settings app might not have.
//!
//! WHY the per-category verdicts are part of the result and not a side note:
//! "no files match" and "the file index is unavailable" are different
//! answers, and only the second one names a reason. A search that says "no
//! results" when the platform refused the listing is a search that lies about
//! the platform.

/// What kind of thing a result is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKind {
    /// An installed package.
    App,
    /// One of the platform's settings surfaces.
    Settings,
    /// A file in the shared storage.
    File,
}

impl SearchKind {
    /// The chip the result row carries.
    pub const fn label(self) -> &'static [u8] {
        match self {
            SearchKind::App => b"App",
            SearchKind::Settings => b"Setting",
            SearchKind::File => b"File",
        }
    }

    /// The order the kinds are ranked in when their scores are equal.
    pub const fn order(self) -> u8 {
        match self {
            SearchKind::App => 0,
            SearchKind::Settings => 1,
            SearchKind::File => 2,
        }
    }
}

/// The longest title an entry may carry.
pub const TITLE_MAX: usize = 40;
/// The longest subtitle an entry may carry.
pub const SUBTITLE_MAX: usize = 40;
/// The longest target an entry may carry.
pub const TARGET_MAX: usize = 48;
/// The longest keyword list an entry may carry.
pub const KEYWORDS_MAX: usize = 96;

/// One indexed thing.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    /// What kind of thing this is.
    pub kind: SearchKind,
    title: [u8; TITLE_MAX],
    title_len: u8,
    subtitle: [u8; SUBTITLE_MAX],
    subtitle_len: u8,
    target: [u8; TARGET_MAX],
    target_len: u8,
    keywords: [u8; KEYWORDS_MAX],
    keywords_len: u8,
}

impl Entry {
    /// An entry with a title, a subtitle, a target and no extra keywords.
    pub fn new(
        kind: SearchKind,
        title: &[u8],
        subtitle: &[u8],
        target: &[u8],
    ) -> Entry {
        let mut entry = Entry {
            kind,
            title: [0; TITLE_MAX],
            title_len: 0,
            subtitle: [0; SUBTITLE_MAX],
            subtitle_len: 0,
            target: [0; TARGET_MAX],
            target_len: 0,
            keywords: [0; KEYWORDS_MAX],
            keywords_len: 0,
        };
        entry.set_title(title);
        entry.set_subtitle(subtitle);
        entry.set_target(target);
        entry
    }

    /// Adds the words this entry should also be found by.
    ///
    /// WHY the words are part of the entry: a settings section is found by the
    /// words a person uses for it -- "wifi" has to reach the Network section
    /// even though the section is called "Network" -- and those words are a
    /// property of the surface, not of the search.
    pub fn with_keywords(mut self, keywords: &[u8]) -> Entry {
        let len = keywords.len().min(KEYWORDS_MAX);
        self.keywords[..len].copy_from_slice(&keywords[..len]);
        self.keywords_len = len as u8;
        self
    }

    fn set_title(&mut self, title: &[u8]) {
        let len = title.len().min(TITLE_MAX);
        self.title[..len].copy_from_slice(&title[..len]);
        self.title_len = len as u8;
    }

    fn set_subtitle(&mut self, subtitle: &[u8]) {
        let len = subtitle.len().min(SUBTITLE_MAX);
        self.subtitle[..len].copy_from_slice(&subtitle[..len]);
        self.subtitle_len = len as u8;
    }

    fn set_target(&mut self, target: &[u8]) {
        let len = target.len().min(TARGET_MAX);
        self.target[..len].copy_from_slice(&target[..len]);
        self.target_len = len as u8;
    }

    /// What the row is called.
    pub fn title(&self) -> &[u8] {
        &self.title[..self.title_len as usize]
    }

    /// The line under the title.
    pub fn subtitle(&self) -> &[u8] {
        &self.subtitle[..self.subtitle_len as usize]
    }

    /// What activating the row opens.
    pub fn target(&self) -> &[u8] {
        &self.target[..self.target_len as usize]
    }

    /// The extra words this entry is found by.
    pub fn keywords(&self) -> &[u8] {
        &self.keywords[..self.keywords_len as usize]
    }
}

/// The subtitle of a file row: the app it opens, then where the file is.
///
/// WHY this lives with the entry it fills: `/storage` itself and the folders
/// one level into it push the same kind of row, and while the rule was written
/// out twice a path cut at the buffer's edge was shortened in one copy and not
/// the other. Both rows then read `/storage/downloads/preview_clip.p`, which
/// names a file the storage does not hold -- a cut that passes itself off as a
/// shorter real name. Wherever the path came from, a cut has to look like a cut.
///
/// Returns the length written, which never exceeds `buf`.
pub fn file_row_subtitle(path: &[u8], buf: &mut [u8; SUBTITLE_MAX]) -> usize {
    const FILES_APP: &[u8] = b"Files: ";
    let mut at = 0usize;
    for &byte in FILES_APP {
        if at >= buf.len() {
            break;
        }
        buf[at] = byte;
        at += 1;
    }
    let room = buf.len().saturating_sub(at);
    if path.len() > room {
        let keep = room.saturating_sub(3);
        buf[at..at + keep].copy_from_slice(&path[..keep]);
        buf[at + keep..at + keep + 3].copy_from_slice(b"...");
        at + keep + 3
    } else {
        buf[at..at + path.len()].copy_from_slice(path);
        at + path.len()
    }
}

/// Whether a category could be read at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    /// The platform answered the query about this category.
    Ready,
    /// The platform did not answer, and the reason says why.
    Unavailable(&'static str),
}

/// The status of the platform's system AI and embedding service for search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiEmbeddingStatus {
    /// Semantic embedding inference service is bound and operational.
    Available {
        abi_version: u16,
    },
    /// The AI service or weights are absent; using token prefix matching fallback.
    Unavailable(&'static str),
}

impl Default for AiEmbeddingStatus {
    fn default() -> Self {
        Self::Unavailable("token prefix matching")
    }
}

impl AiEmbeddingStatus {
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available { .. })
    }

    pub const fn fallback_strategy(&self) -> &'static str {
        "token prefix matching"
    }

    pub const fn detail(&self) -> &'static str {
        match self {
            Self::Available { .. } => "embedding service online",
            Self::Unavailable(reason) => reason,
        }
    }
}

/// What a category says about the query.
///
/// WHY the three cases and not a count: "3 matches" and "no match" and "the
/// platform would not answer" are three different states of the world, and
/// the last one is the one that has to name its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryStatus {
    /// The category was searched and this many entries matched.
    Matched(usize),
    /// The category was searched and nothing matched.
    NoMatch,
    /// The category could not be searched; the reason says why.
    Unavailable(&'static str),
}

/// The ranker seam.
///
/// WHY this is a trait and not a function: a semantic ranker asks the model
/// manager's embedding service for a vector for the query and for each
/// candidate, and scores the candidate by the similarity of the two. That is a
/// different *source* for the score, not a different kind of entry, so the
/// index, the result list and the screen do not change when it arrives -- only
/// the implementation of this trait does.
///
/// WHY the exact-match ranker stays the default: a model that is not running
/// has to leave the search working, and a search that silently stops
/// answering is worse than one that answers exactly.
pub trait Ranker {
    /// The score of `entry` for `query`, or `None` when it does not match.
    fn score(&self, query: &[u8], entry: &Entry) -> Option<u32>;
}

/// The ranker in use today: the query is matched against the entry's own
/// words.
#[derive(Debug, Clone, Copy, Default)]
pub struct ExactMatch;

/// The score of an entry whose title is exactly the query.
pub const SCORE_EXACT: u32 = 1000;
/// The score of an entry whose title starts with the query.
pub const SCORE_PREFIX: u32 = 800;
/// The score of an entry with a word that starts with the query.
pub const SCORE_WORD: u32 = 600;
/// The score of an entry whose title contains the query.
pub const SCORE_CONTAINS: u32 = 400;
/// The score of an entry whose keywords match the query.
pub const SCORE_KEYWORD: u32 = 300;
/// The score of an entry whose target contains the query.
pub const SCORE_TARGET: u32 = 200;
/// The score of an entry whose subtitle contains the query.
pub const SCORE_SUBTITLE: u32 = 100;

impl Ranker for ExactMatch {
    fn score(&self, query: &[u8], entry: &Entry) -> Option<u32> {
        if query.is_empty() {
            return None;
        }
        let title = entry.title();
        if eq_ignore_case(title, query) {
            return Some(SCORE_EXACT);
        }
        if starts_with_ignore_case(title, query) {
            return Some(SCORE_PREFIX);
        }
        if word_starts_with(title, query) {
            return Some(SCORE_WORD);
        }
        if contains_ignore_case(title, query) {
            return Some(SCORE_CONTAINS);
        }
        if keyword_matches(entry.keywords(), query) {
            return Some(SCORE_KEYWORD);
        }
        if contains_ignore_case(entry.target(), query) {
            return Some(SCORE_TARGET);
        }
        if contains_ignore_case(entry.subtitle(), query) {
            return Some(SCORE_SUBTITLE);
        }
        None
    }
}

/// A search ranker that combines token prefix matching with system AI embedding status.
///
/// When the system AI inference service and weights are available, it incorporates
/// embedding vector cosine similarity into candidate scoring. When unavailable or absent,
/// it falls back strictly and honestly to deterministic token prefix matching.
#[derive(Debug, Clone, Copy, Default)]
pub struct SemanticRanker {
    pub exact: ExactMatch,
    pub ai_status: AiEmbeddingStatus,
}

impl SemanticRanker {
    pub const fn new(status: AiEmbeddingStatus) -> Self {
        Self {
            exact: ExactMatch,
            ai_status: status,
        }
    }
}

impl Ranker for SemanticRanker {
    fn score(&self, query: &[u8], entry: &Entry) -> Option<u32> {
        let base_score = self.exact.score(query, entry);
        if !self.ai_status.is_available() {
            // Under the Platform Truth Contract, when the AI daemon or weights are absent,
            // we must never fabricate synthetic embeddings or scores. We honestly execute
            // exact / token prefix matching.
            return base_score;
        }

        // Platform Truth Contract: `compute_text_embedding_q7` is a lexical n-gram hash
        // projection into 128-D space, not a neural embedding from weights. True neural
        // retrieval requires IPC vector query to `aid` (`0xFFFF_000C`). When AI status
        // is marked available, this calculates similarity using the deterministic n-gram projection.
        let mut query_vec = [0i8; 128];
        let mut entry_vec = [0i8; 128];
        interos_std::ai::compute_text_embedding_q7(query, &mut query_vec);
        interos_std::ai::compute_text_embedding_q7(entry.title(), &mut entry_vec);
        let sim = interos_std::ai::cosine_similarity_q7(&query_vec, &entry_vec);

        if sim > 0 {
            let sim_boost = (sim as u32 * 400) / 32767;
            if let Some(base) = base_score {
                Some(base.saturating_add(sim_boost))
            } else if sim > 8000 {
                // Semantic candidate hit for semantically close query
                Some(250 + sim_boost)
            } else {
                None
            }
        } else {
            base_score
        }
    }
}

/// Compares two byte runs without regard to case.
pub fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Whether `haystack` starts with `needle`, ignoring case.
pub fn starts_with_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Whether `haystack` contains `needle`, ignoring case.
pub fn contains_ignore_case(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    let mut at = 0usize;
    while at + needle.len() <= haystack.len() {
        if haystack[at..at + needle.len()].eq_ignore_ascii_case(needle) {
            return true;
        }
        at += 1;
    }
    false
}

/// Whether a word of `haystack` starts with `needle`.
///
/// WHY the word boundary: a search for "set" should find "Settings" and
/// "Wallpaper", and not "Sunset". A plain substring test would answer yes to
/// the second as well, and every query would match half the index.
pub fn word_starts_with(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    let mut at = 0usize;
    while at < haystack.len() {
        if is_word_start(haystack, at) && starts_with_ignore_case(&haystack[at..], needle) {
            return true;
        }
        at += 1;
    }
    false
}

/// Whether the byte at `at` begins a word.
fn is_word_start(text: &[u8], at: usize) -> bool {
    if at == 0 {
        return true;
    }
    let previous = text[at - 1];
    previous == b' ' || previous == b'-' || previous == b'_' || previous == b'.'
}

/// Whether any of the space-separated keywords matches the query.
fn keyword_matches(keywords: &[u8], query: &[u8]) -> bool {
    let mut at = 0usize;
    while at < keywords.len() {
        let mut end = at;
        while end < keywords.len() && keywords[end] != b' ' {
            end += 1;
        }
        let word = &keywords[at..end];
        if eq_ignore_case(word, query) || starts_with_ignore_case(word, query) {
            return true;
        }
        at = end + 1;
    }
    false
}

/// How many entries the index holds at most.
pub const MAX_ENTRIES: usize = 40;
/// How many results a search returns at most.
pub const MAX_HITS: usize = 20;

/// The list the search runs against.
#[derive(Debug, Clone, Copy)]
pub struct Index {
    entries: [Entry; MAX_ENTRIES],
    count: usize,
    apps: SourceState,
    settings: SourceState,
    files: SourceState,
    ai: SourceState,
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Index {
    /// An empty index whose sources have all answered.
    pub const fn new() -> Index {
        Index {
            entries: [Entry {
                kind: SearchKind::App,
                title: [0; TITLE_MAX],
                title_len: 0,
                subtitle: [0; SUBTITLE_MAX],
                subtitle_len: 0,
                target: [0; TARGET_MAX],
                target_len: 0,
                keywords: [0; KEYWORDS_MAX],
                keywords_len: 0,
            }; MAX_ENTRIES],
            count: 0,
            apps: SourceState::Ready,
            settings: SourceState::Ready,
            files: SourceState::Ready,
            ai: SourceState::Unavailable("token prefix matching"),
        }
    }

    /// Empties the index, keeping the source states.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// Adds an entry, if there is room for it.
    pub fn push(&mut self, entry: Entry) -> bool {
        if self.count >= MAX_ENTRIES {
            return false;
        }
        self.entries[self.count] = entry;
        self.count += 1;
        true
    }

    /// How many entries the index holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The entry at `index`.
    pub fn get(&self, index: usize) -> Option<&Entry> {
        if index < self.count {
            Some(&self.entries[index])
        } else {
            None
        }
    }

    /// Records whether the package registry answered.
    pub fn set_apps_state(&mut self, state: SourceState) {
        self.apps = state;
    }

    /// Records whether the settings catalogue answered.
    pub fn set_settings_state(&mut self, state: SourceState) {
        self.settings = state;
    }

    /// Records whether the file listing answered.
    pub fn set_files_state(&mut self, state: SourceState) {
        self.files = state;
    }

    /// Records whether the system AI embedding service answered.
    pub fn set_ai_state(&mut self, state: SourceState) {
        self.ai = state;
    }

    /// The state of the package registry.
    pub fn apps_state(&self) -> SourceState {
        self.apps
    }

    /// The state of the settings catalogue.
    pub fn settings_state(&self) -> SourceState {
        self.settings
    }

    /// The state of the file listing.
    pub fn files_state(&self) -> SourceState {
        self.files
    }

    /// The state of the system AI and embedding service.
    pub fn ai_state(&self) -> SourceState {
        self.ai
    }

    /// Runs `query` across the index and ranks what it finds.
    pub fn search<R: Ranker>(&self, query: &[u8], ranker: &R) -> Results {
        let mut results = Results::new();
        let mut apps = 0usize;
        let mut settings = 0usize;
        let mut files = 0usize;

        for index in 0..self.count {
            let entry = &self.entries[index];
            let Some(score) = ranker.score(query, entry) else {
                continue;
            };
            match entry.kind {
                SearchKind::App => apps += 1,
                SearchKind::Settings => settings += 1,
                SearchKind::File => files += 1,
            }
            results.insert(index as u16, score, entry.kind);
        }

        results.apps = self.category_status(self.apps, apps);
        results.settings = self.category_status(self.settings, settings);
        results.files = self.category_status(self.files, files);
        let total_matches = apps + settings + files;
        results.ai = self.category_status(self.ai, total_matches);
        results
    }

    fn category_status(&self, state: SourceState, matched: usize) -> CategoryStatus {
        match state {
            SourceState::Unavailable(reason) => CategoryStatus::Unavailable(reason),
            SourceState::Ready => {
                if matched > 0 {
                    CategoryStatus::Matched(matched)
                } else {
                    CategoryStatus::NoMatch
                }
            }
        }
    }
}

/// One result.
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    /// Where the result sits in the index.
    pub index: u16,
    /// The score the ranker gave it.
    pub score: u32,
    /// What kind of thing it is.
    pub kind: SearchKind,
}

/// What a search returned.
#[derive(Debug, Clone, Copy)]
pub struct Results {
    /// The hits, best first.
    pub hits: [Hit; MAX_HITS],
    /// How many hits there are.
    pub count: usize,
    /// What the app category says about the query.
    pub apps: CategoryStatus,
    /// What the settings category says about the query.
    pub settings: CategoryStatus,
    /// What the file category says about the query.
    pub files: CategoryStatus,
    /// What the AI category says about the query and embedding availability.
    pub ai: CategoryStatus,
}

impl Default for Results {
    fn default() -> Self {
        Self::new()
    }
}

impl Results {
    /// A result set with nothing in it.
    pub const fn new() -> Results {
        Results {
            hits: [Hit { index: 0, score: 0, kind: SearchKind::App }; MAX_HITS],
            count: 0,
            apps: CategoryStatus::NoMatch,
            settings: CategoryStatus::NoMatch,
            files: CategoryStatus::NoMatch,
            ai: CategoryStatus::Unavailable("token prefix matching"),
        }
    }

    /// The hit at `rank`.
    pub fn hit(&self, rank: usize) -> Option<&Hit> {
        if rank < self.count {
            Some(&self.hits[rank])
        } else {
            None
        }
    }

    /// Puts a hit in the list, keeping the list ordered by score.
    fn insert(&mut self, index: u16, score: u32, kind: SearchKind) {
        let hit = Hit { index, score, kind };
        if self.count >= MAX_HITS {
            // WHY the best hit is kept and the rest are dropped: a search that
            // listed more than the surface can show would have to leave the
            // ranking to the display, and the display shows the top of the
            // list anyway.
            if score <= self.hits[MAX_HITS - 1].score {
                return;
            }
            self.hits[MAX_HITS - 1] = hit;
        } else {
            self.hits[self.count] = hit;
            self.count += 1;
        }
        self.bubble();
    }

    /// Moves the last hit into its place, best first.
    ///
    /// WHY a bubble and not a sort: the list is at most twenty long and is
    /// built one hit at a time, so the one element that can be out of place
    /// is the one just added.
    fn bubble(&mut self) {
        let mut at = self.count;
        while at > 1 {
            let upper = self.hits[at - 2];
            let lower = self.hits[at - 1];
            if self.ordering(upper) >= self.ordering(lower) {
                break;
            }
            self.hits[at - 2] = lower;
            self.hits[at - 1] = upper;
            at -= 1;
        }
    }

    /// The sort key of a hit.
    ///
    /// WHY the kind is in the key: two things can score the same -- an app
    /// called "Files" and a file called "Files" -- and the user reading the
    /// list should meet the app first, because that is the thing people mean
    /// when they type a name.
    fn ordering(&self, hit: Hit) -> u32 {
        (hit.score << 8) | (hit.kind.order() as u32)
    }
}

// -----------------------------------------------------------------------------
// The lines the search surface may ink
// -----------------------------------------------------------------------------

/// How many rows the search surface's list region can ink.
///
/// WHY this reads [`crate::layout::SEARCH_MAX_ROWS`] and not its own number: the
/// row the painter inks and the row a tap resolves are the same rectangle only
/// while both read one count, and the band a keystroke repaints is measured
/// against the layout's number by `apps/launcher/src/tests.rs`.
pub const MAX_LIST_ROWS: usize = crate::layout::SEARCH_MAX_ROWS;

/// The one hint the query field carries while nobody has typed a needle.
///
/// WHY the copy lives here: it is a *hint*, not a result, and the rule that keeps
/// it a hint -- inked only while the field is empty, and only inside the field --
/// has to be in one place a host test can reach. A surface that writes its own
/// copy of that rule is a surface that can ink it in a row slot.
pub const FIELD_HINT: &[u8] = b"Search apps, settings and files";

/// One row the list region holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListRow {
    /// The rank of the hit this row paints; a tap on the row opens this hit.
    pub rank: usize,
    /// Where the entry this row names sits in the index.
    pub entry: usize,
}

/// The rows the list region holds for one answer.
///
/// WHY a type and not a loop in the painter: the surface used to walk the ranking
/// inside its own `build`, stop at the first hit the index could not name, and
/// hand the tap path a *different* number -- the ranking's own count -- to hit-test
/// against. So the panel inked two rows, answered taps on six, and the slots under
/// the ink held nothing but whatever the last frame left there. One list, built
/// from the answer and the index together, is the only arrangement where the paint
/// and the tap cannot disagree about which rows exist.
#[derive(Clone, Copy)]
pub struct ListRows {
    rows: [Option<ListRow>; MAX_LIST_ROWS],
    len: usize,
}

impl ListRows {
    /// A list with nothing in it.
    pub const fn none() -> ListRows {
        ListRows {
            rows: [None; MAX_LIST_ROWS],
            len: 0,
        }
    }

    /// The rows the surface has to ink for this answer.
    ///
    /// WHY a hit whose entry the index cannot name is skipped rather than taken as
    /// the end of the list: the hits under it are still the answer the ranking
    /// returned, and a list that stops at a hole hides results behind it -- and
    /// WHY it is skipped rather than inked with a stale entry: a row painted from
    /// an entry the index no longer holds is ink standing over a region the service
    /// gave nothing for, which is the one thing this surface must not do.
    pub fn build(results: &Results, index: &Index) -> ListRows {
        let mut list = ListRows::none();
        let mut rank = 0usize;
        while rank < results.count && list.len < MAX_LIST_ROWS {
            let Some(hit) = results.hit(rank) else {
                break;
            };
            if index.get(hit.index as usize).is_some() {
                list.rows[list.len] = Some(ListRow {
                    rank,
                    entry: hit.index as usize,
                });
                list.len += 1;
            }
            rank += 1;
        }
        list
    }

    /// How many rows the list holds.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the list holds no row at all.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The row in the list's `slot`th place.
    pub fn get(&self, slot: usize) -> Option<ListRow> {
        if slot < self.len {
            self.rows[slot]
        } else {
            None
        }
    }

    /// The rank of the row (`x`, `y`) lands on, or `None` for a slot the list does
    /// not hold.
    ///
    /// WHY the rank and not the slot: the two are the same number only while every
    /// hit names an entry. Once a hit is skipped the third inked row is the answer
    /// to the person's *third* result, and opening the hit at slot three would open
    /// something else.
    pub fn row_at(&self, x: usize, y: usize) -> Option<usize> {
        let mut slot = 0usize;
        while slot < self.len {
            if row_rect(slot).contains(x, y) {
                return self.get(slot).map(|row| row.rank);
            }
            slot += 1;
        }
        None
    }
}

/// The rectangle of the list's `slot`th row.
///
/// WHY the slot and not the rank: the list is inked top-down in the slots it holds,
/// so the third inked row sits in the third slot whatever rank the ranking gave it
/// -- and the tap has to be answered from the same slot the painter filled.
pub fn row_rect(slot: usize) -> crate::layout::Rect {
    crate::layout::Rect::new(
        crate::layout::SEARCH_FIELD.x,
        crate::layout::search_row_top(slot),
        crate::layout::SEARCH_FIELD.w,
        crate::layout::SEARCH_ROW_H - 8,
    )
}

/// The field's hint: the copy it carries while nobody has typed a needle.
///
/// WHY `None` the moment a needle exists: a hint inked behind a typed query reads
/// as a suggestion the person did not ask for, sitting on top of what they typed.
pub const fn field_hint(needle_len: usize) -> Option<&'static [u8]> {
    if needle_len == 0 {
        Some(FIELD_HINT)
    } else {
        None
    }
}

/// Whether the line under the verdict card may be inked.
///
/// WHAT that line says: "Tap a result to open it. Backspace edits the query." Both
/// halves are promises about state the surface may not hold -- a row to tap, and a
/// query to edit -- so the line is inked only while both are true.
///
/// WHY it is not inked in the empty state at all: the alternative is to invent a
/// sentence for the region the answer left empty, and an invented sentence over
/// dead space is what the person reads as a result they did not get. The headline
/// above the list already says the one thing the surface knows -- "Type to search"
/// while nobody has typed, "No results" when a needle brought nothing back.
pub const fn foot_line_applies(needle_len: usize, rows: usize) -> bool {
    needle_len > 0 && rows > 0
}
