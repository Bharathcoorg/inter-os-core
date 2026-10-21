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

/// Whether a category could be read at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceState {
    /// The platform answered the query about this category.
    Ready,
    /// The platform did not answer, and the reason says why.
    Unavailable(&'static str),
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
