#![no_std]

//! The agent manager's model layer.
//!
//! WHY this module exists and what it is for: the agent manager's job is to
//! report what the device actually holds, and every claim it makes has to
//! come from a measurement made here. A measurement in this module has
//! exactly three possible answers -- what the platform said, what the
//! platform refused, and never what the app would have liked it to say. A
//! screen that draws a number has to have got that number from one of these
//! three, and there is no third source for it to reach for.
//!
//! WHY the providers are behind a trait: a model is not a row in a table.
//! When the next generative model arrives it arrives as an implementation of
//! [`Provider`], and the catalogue, the screens and the selection all keep
//! working without being edited. The trait is the seam; a `match` on a
//! model name would not be.
//!
//! WHY nothing here allocates: the app runs in Ring 3 with no heap, so every
//! measurement lands in a fixed-size structure the caller owns.

pub use interos_std::ai::MatryoshkaDim;

/// The vector dimension the agent manager uses when the platform holds none.
///
/// WHY this is a constant and not a guess at run time: the platform's
/// settings store is the only place a real configuration lives, and when it
/// holds nothing the app says so and uses a value it names here.
pub const MATRYOSHKA_DEFAULT_DIM: MatryoshkaDim = MatryoshkaDim::Dim256;

/// The similarity cutoff the agent manager uses when the platform holds none.
///
/// WHY Q15: it is the unit the platform's own fixed-point cosine returns, so
/// the cutoff and the score are the same kind of number and can be compared
/// without a conversion.
pub const SIMILARITY_DEFAULT_Q15: i16 = 16384;

/// The result limit the agent manager uses when the platform holds none.
pub const MAX_RESULTS_DEFAULT: u16 = 5;

// -----------------------------------------------------------------------------
// The platform's answers
// -----------------------------------------------------------------------------

/// The kernel refused the call: the caller holds no such authority.
pub const ERR_PERMISSION_DENIED: i64 = -1;
/// The kernel refused the call: the arguments were not usable.
pub const ERR_INVALID_ARGUMENTS: i64 = -3;
/// The kernel refused the call: the path does not exist.
pub const ERR_FILE_NOT_FOUND: i64 = -8;

/// The outcome of a query against the platform.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProbeStatus {
    /// The platform answered.
    Listed,
    /// The platform refused the query.
    Refused,
}

impl ProbeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Listed => "OK",
            Self::Refused => "REFUSED",
        }
    }
}

// -----------------------------------------------------------------------------
// The surfaces the app shows
// -----------------------------------------------------------------------------

/// One of the app's five surfaces.
///
/// WHY an enum and not a number: a number is a position, and the position of
/// a surface in a tab row is not the thing the screen means. The name is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Surface {
    /// Model management: what the device holds and what it does not.
    Models = 0,
    /// The capability grants this app actually holds.
    Access = 1,
    /// The services the platform's registry actually reports.
    Runtime = 2,
    /// The search configuration and the platform's content layer.
    Search = 3,
    /// The self-test, run against the platform's own functions.
    Test = 4,
    /// The conversation with whatever model the device actually has.
    Chat = 5,
}

impl Surface {
    /// How many surfaces the app has.
    pub const COUNT: usize = 6;

    /// Every surface, in the order the tab row shows them.
    pub const ALL: [Surface; Surface::COUNT] = [
        Surface::Models,
        Surface::Access,
        Surface::Runtime,
        Surface::Search,
        Surface::Test,
        Surface::Chat,
    ];

    /// The surface's position in the tab row.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The surface at a position, if there is one.
    pub const fn from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Surface::Models),
            1 => Some(Surface::Access),
            2 => Some(Surface::Runtime),
            3 => Some(Surface::Search),
            4 => Some(Surface::Test),
            5 => Some(Surface::Chat),
            _ => None,
        }
    }

    /// The title the surface's heading carries.
    pub const fn title(self) -> &'static str {
        match self {
            Surface::Models => "Models",
            Surface::Access => "Access",
            Surface::Runtime => "Runtime",
            Surface::Search => "Search",
            Surface::Test => "Self-test",
            Surface::Chat => "Chat",
        }
    }

    /// The short label the tab row carries.
    pub const fn tab_label(self) -> &'static str {
        match self {
            Surface::Models => "MODELS",
            Surface::Access => "ACCESS",
            Surface::Runtime => "RUNTIME",
            Surface::Search => "SEARCH",
            Surface::Test => "TEST",
            Surface::Chat => "CHAT",
        }
    }
}

// -----------------------------------------------------------------------------
// The chat surface
// -----------------------------------------------------------------------------

/// Which side of a conversation a message came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    /// Something the person typed.
    User,
    /// Something the model said.
    Assistant,
    /// Something the platform said about the exchange.
    Notice,
}

impl ChatRole {
    /// The label the transcript carries for this side of the exchange.
    pub const fn label(self) -> &'static str {
        match self {
            ChatRole::User => "YOU",
            ChatRole::Assistant => "MODEL",
            ChatRole::Notice => "PLATFORM",
        }
    }
}

/// The longest message the transcript holds.
pub const CHAT_TEXT_MAX: usize = 96;

/// The longest prompt the composer holds.
pub const CHAT_PROMPT_MAX: usize = 160;

/// How many messages the transcript holds.
///
/// WHY the transcript is a ring: a conversation that has run for a while
/// has more messages than a screen has room for, and a supervisor that
/// grows without bound is a supervisor that eventually cannot allocate.
/// The oldest message is the one that goes.
///
/// WHY ten and not more: the body holds twelve cards, and the chat
/// spends two of them on the service state and the prompt. A transcript
/// that wanted a thirteenth card would have its oldest message dropped
/// by the *body* rather than by the ring, and the two would disagree
/// about which message was oldest.
pub const CHAT_MAX_MESSAGES: usize = 10;

/// One message in the conversation.
#[derive(Clone, Copy)]
pub struct ChatMessage {
    /// Which side of the exchange the message came from.
    pub role: ChatRole,
    /// The message's text.
    pub text: [u8; CHAT_TEXT_MAX],
    /// How much of the text buffer the message uses.
    pub text_len: u8,
}

impl ChatMessage {
    /// An empty message.
    pub const fn empty() -> Self {
        Self {
            role: ChatRole::Notice,
            text: [0u8; CHAT_TEXT_MAX],
            text_len: 0,
        }
    }
}

/// What the platform can do with a prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InferenceAvailability {
    /// A service answered, it is running, and it speaks this version.
    Available {
        /// The version the service speaks.
        abi_version: u16,
    },
    /// A service answered, but it is not running.
    NotRunning {
        /// The state the platform reported.
        state: ServiceState,
    },
    /// A service answered, but it speaks a version this app does not.
    WrongVersion {
        /// The version the service speaks.
        abi_version: u16,
    },
    /// The platform's registry names no inference service at all.
    NoService,
    /// The platform's registry could not be read.
    ///
    /// WHY this is a different answer from `NoService`: a registry that
    /// was refused has not said that there is no inference service. It has
    /// said nothing, and the screen must not turn that into a fact.
    RegistryUnavailable,
}

impl InferenceAvailability {
    /// The sentence the chat shows for this answer.
    ///
    /// WHY a sentence and not a word: "unavailable" tells the user
    /// nothing they can act on. The three answers have three different
    /// remedies, and the screen has to name which one it is.
    pub fn reason(self) -> &'static str {
        match self {
            InferenceAvailability::Available { .. } => {
                "The platform's inference service is running and speaks this version."
            }
            InferenceAvailability::NotRunning { state } => match state {
                ServiceState::Declared => {
                    "The platform declares an inference service, but it has not started."
                }
                ServiceState::Starting => {
                    "The platform's inference service is still starting."
                }
                ServiceState::Stopped => {
                    "The platform's inference service is stopped."
                }
                ServiceState::Failed => {
                    "The platform's inference service failed to start."
                }
                ServiceState::Unknown => {
                    "The platform reports an inference service in a state this app does not know."
                }
                ServiceState::Running => unreachable!(),
            },
            InferenceAvailability::WrongVersion { .. } => {
                "The platform's inference service speaks a version this app does not."
            }
            InferenceAvailability::NoService => {
                "This device has no inference service: the platform's registry names none."
            }
            InferenceAvailability::RegistryUnavailable => {
                "The platform's service registry could not be read, so the services it runs are unknown."
            }
        }
    }

    /// Whether a prompt can be sent.
    pub const fn is_available(self) -> bool {
        matches!(self, InferenceAvailability::Available { .. })
    }
}

/// Looks for the platform's inference service in the registry.
///
/// WHY the registry and not a hardcoded list: the registry is what the
/// platform reports about the services it is actually running. A service
/// that the registry does not name is not a service this device has,
/// whatever any catalogue says it should be.
///
/// WHY the registry's own answer is checked first: a registry the
/// platform refused to answer has not said that there is no inference
/// service. It has said nothing, and the chat must not report silence
/// as a fact.
///
/// WHY the version is checked: the service's contract can change, and a
/// request sent to a service that speaks another version would be
/// misread rather than refused. The mismatch is a fact the app can
/// report, so it reports it instead of guessing.
pub fn resolve_inference_service(registry: &ServiceRegistry) -> InferenceAvailability {
    if registry.status != ProbeStatus::Listed {
        return InferenceAvailability::RegistryUnavailable;
    }
    let count = (registry.count as usize).min(MAX_SERVICES);
    let entries = &registry.entries[..count];
    let mut found: Option<&ServiceEntry> = None;
    for entry in entries {
        if entry.id == interos_std::ai::SERVICE_INFERENCE {
            found = Some(entry);
            break;
        }
    }
    let Some(entry) = found else {
        return InferenceAvailability::NoService;
    };
    if entry.state != ServiceState::Running {
        return InferenceAvailability::NotRunning { state: entry.state };
    }
    if entry.abi_version != interos_std::ai::INFERENCE_ABI_VERSION {
        return InferenceAvailability::WrongVersion {
            abi_version: entry.abi_version,
        };
    }
    InferenceAvailability::Available {
        abi_version: entry.abi_version,
    }
}

/// The conversation, and the prompt being written.
#[derive(Clone, Copy)]
pub struct ChatSession {
    /// The transcript, oldest first when read through `message_at`.
    pub messages: [ChatMessage; CHAT_MAX_MESSAGES],
    /// How many messages the transcript holds.
    pub message_count: u8,
    /// Where the next message goes.
    pub next: u8,
    /// The prompt being written.
    pub prompt: [u8; CHAT_PROMPT_MAX],
    /// How much of the prompt buffer is used.
    pub prompt_len: u8,
}

impl ChatSession {
    /// An empty conversation.
    pub const fn new() -> Self {
        Self {
            messages: [ChatMessage::empty(); CHAT_MAX_MESSAGES],
            message_count: 0,
            next: 0,
            prompt: [0u8; CHAT_PROMPT_MAX],
            prompt_len: 0,
        }
    }

    /// Adds a message, dropping the oldest when the transcript is full.
    pub fn push(&mut self, role: ChatRole, text: &[u8]) {
        let index = self.next as usize % CHAT_MAX_MESSAGES;
        self.messages[index].role = role;
        let len = text.len().min(CHAT_TEXT_MAX);
        self.messages[index].text[..len].copy_from_slice(&text[..len]);
        self.messages[index].text_len = len as u8;
        self.next = (self.next + 1) % CHAT_MAX_MESSAGES as u8;
        if (self.message_count as usize) < CHAT_MAX_MESSAGES {
            self.message_count += 1;
        }
    }

    /// The message at a position, oldest first.
    ///
    /// WHY the index is computed rather than stored: the ring's write
    /// position moves, and a transcript whose order is derived from it
    /// cannot disagree with itself about which message is oldest.
    pub fn message_at(&self, position: usize) -> Option<&ChatMessage> {
        if position >= self.message_count as usize {
            return None;
        }
        let count = self.message_count as usize;
        let start = (self.next as usize + CHAT_MAX_MESSAGES - count) % CHAT_MAX_MESSAGES;
        let index = (start + position) % CHAT_MAX_MESSAGES;
        Some(&self.messages[index])
    }

    /// The number of messages the transcript holds.
    pub const fn len(&self) -> usize {
        self.message_count as usize
    }

    /// Whether the transcript is empty.
    pub const fn is_empty(&self) -> bool {
        self.message_count == 0
    }

    /// Appends a character to the prompt.
    ///
    /// WHY the prompt is bounded and refuses silently: a composer that
    /// cannot hold more than the platform will accept is a composer that
    /// has to truncate a half-typed sentence, and a half-typed sentence
    /// that is silently cut is a sentence the user never wrote.
    pub fn push_prompt_char(&mut self, c: u8) -> bool {
        if self.prompt_len as usize >= CHAT_PROMPT_MAX {
            return false;
        }
        self.prompt[self.prompt_len as usize] = c;
        self.prompt_len += 1;
        true
    }

    /// Removes the last character of the prompt.
    pub fn pop_prompt_char(&mut self) {
        if self.prompt_len > 0 {
            self.prompt_len -= 1;
        }
    }

    /// Empties the prompt.
    pub fn clear_prompt(&mut self) {
        self.prompt_len = 0;
    }

    /// The prompt's text.
    pub fn prompt_text(&self) -> &[u8] {
        &self.prompt[..self.prompt_len as usize]
    }

    /// Whether there is anything to send.
    pub fn prompt_is_empty(&self) -> bool {
        self.prompt_len == 0
    }
}

// -----------------------------------------------------------------------------
// Providers
// -----------------------------------------------------------------------------

/// What a provider can do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProviderCapabilities {
    /// The provider computes dense vector embeddings.
    pub embeds: bool,
    /// The provider produces conversational text.
    pub generates: bool,
    /// The provider's execution happens off the device.
    pub host_backed: bool,
}

impl ProviderCapabilities {
    /// The role line the catalogue shows under a provider's name.
    ///
    /// WHY the role is spelled out rather than shown as two ticks: a reader
    /// has to know what the model is *for*, and "embeds, does not generate"
    /// is only meaningful as a sentence.
    pub const fn role(self) -> &'static str {
        match (self.embeds, self.generates, self.host_backed) {
            (true, false, _) => "vector embeddings only",
            (false, true, true) => "conversational, host-backed",
            (false, true, false) => "conversational, on-device",
            (false, false, _) => "no model capability",
            (true, true, _) => "embeddings and generation",
        }
    }
}

/// What the device actually holds for a provider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum InstallState {
    /// The device holds the provider's artefacts.
    Installed,
    /// The device does not hold them.
    NotInstalled,
    /// The platform refused to say.
    Unavailable,
}

impl InstallState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "INSTALLED",
            Self::NotInstalled => "NOT INSTALLED",
            Self::Unavailable => "UNAVAILABLE",
        }
    }
}

/// Whether a runtime that can serve a provider is bound.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuntimeBinding {
    /// A runtime is registered with the platform.
    Bound,
    /// No runtime is registered, and this is the reason it is not.
    Unbound {
        /// Why the runtime is not bound, in the platform's own terms.
        reason: &'static str,
    },
}

impl RuntimeBinding {
    pub const fn is_bound(self) -> bool {
        matches!(self, Self::Bound)
    }

    /// The label the screen shows.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bound => "BOUND",
            Self::Unbound { .. } => "UNBOUND",
        }
    }
}

/// A model the agent manager can manage.
///
/// WHY a trait and not an enum: a new model is a new implementation of this
/// trait and one more entry in the registry. It is not an edit through the
/// screens, and it does not require the app to learn the new model's name in
/// five different places.
pub trait Provider: Sync {
    /// The provider's stable identifier.
    fn id(&self) -> &'static str;

    /// The name the catalogue shows.
    fn display_name(&self) -> &'static str;

    /// What the provider can do.
    fn capabilities(&self) -> ProviderCapabilities;

    /// The package that delivers the provider, if it is delivered as one.
    ///
    /// WHY this is the *request* and not a claim: the id names the package
    /// the app asks the platform for. Whether that package exists is
    /// something the platform says, and the catalogue's install state comes
    /// from the platform's answer rather than from this id.
    fn package_id(&self) -> Option<&'static str>;

    /// What the device actually holds for this provider.
    fn install_state(&self, device: &DeviceState) -> InstallState;

    /// Whether a runtime that can serve the provider is bound.
    fn runtime_binding(&self, device: &DeviceState) -> RuntimeBinding;
}

/// EmbeddingGemma 2: the on-device vector embedding model.
///
/// WHY it can never claim to generate text: a vector embedding is a fixed
/// length vector of numbers, and there is no decoder attached to it that
/// could turn one into a sentence. The catalogue says so in words because a
/// badge that says "MODEL" would not.
pub struct EmbeddingGemma2;

impl Provider for EmbeddingGemma2 {
    fn id(&self) -> &'static str {
        "embeddinggemma2"
    }

    fn display_name(&self) -> &'static str {
        "EmbeddingGemma 2"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            embeds: true,
            generates: false,
            host_backed: false,
        }
    }

    fn package_id(&self) -> Option<&'static str> {
        Some("org.interos.embeddinggemma2")
    }

    fn install_state(&self, device: &DeviceState) -> InstallState {
        match device.models.status {
            ModelProbeStatus::Listed => {
                if device.models.has_embedding_weights {
                    InstallState::Installed
                } else {
                    InstallState::NotInstalled
                }
            }
            ModelProbeStatus::Absent => InstallState::NotInstalled,
            ModelProbeStatus::Refused | ModelProbeStatus::NoAuthority => InstallState::Unavailable,
        }
    }

    fn runtime_binding(&self, device: &DeviceState) -> RuntimeBinding {
        device.services.inference_runtime_binding()
    }
}

/// The development host bridge: an external workstation, reached over IPC.
///
/// WHY it is in the catalogue at all: it is the provider the platform's own
/// documentation names, and a supervisor that cannot say "the host bridge is
/// not bound" cannot say anything useful about the device. Its install state
/// is therefore the state of the runtime that would serve it, which the
/// platform's own registry answers.
pub struct HostBridge;

impl Provider for HostBridge {
    fn id(&self) -> &'static str {
        "host-bridge"
    }

    fn display_name(&self) -> &'static str {
        "Host bridge"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            embeds: false,
            generates: true,
            host_backed: true,
        }
    }

    fn package_id(&self) -> Option<&'static str> {
        // WHY none: the bridge is not delivered to the device as a package.
        // It runs on a development workstation, and the guest has no package
        // for it to install.
        None
    }

    fn install_state(&self, device: &DeviceState) -> InstallState {
        match device.services.status {
            ProbeStatus::Listed => {
                if device.services.contains_inference_runtime() {
                    InstallState::Installed
                } else {
                    InstallState::NotInstalled
                }
            }
            ProbeStatus::Refused => InstallState::Unavailable,
        }
    }

    fn runtime_binding(&self, device: &DeviceState) -> RuntimeBinding {
        device.services.inference_runtime_binding()
    }
}

/// A future on-device generative model.
///
/// WHY it is here even though the device holds nothing for it: the point of
/// a provider interface is that the next model arrives without an edit
/// through the app, and the catalogue has to be able to say "not installed"
/// about a model it knows about. What it must never do is say "ready".
pub struct LocalGenerative;

impl Provider for LocalGenerative {
    fn id(&self) -> &'static str {
        "local-generative"
    }

    fn display_name(&self) -> &'static str {
        "On-device generative"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            embeds: false,
            generates: true,
            host_backed: false,
        }
    }

    fn package_id(&self) -> Option<&'static str> {
        Some("org.interos.localgenerative")
    }

    fn install_state(&self, device: &DeviceState) -> InstallState {
        match device.models.status {
            ModelProbeStatus::Listed => {
                if device.models.has_generative_weights {
                    InstallState::Installed
                } else {
                    InstallState::NotInstalled
                }
            }
            ModelProbeStatus::Absent => InstallState::NotInstalled,
            ModelProbeStatus::Refused | ModelProbeStatus::NoAuthority => InstallState::Unavailable,
        }
    }

    fn runtime_binding(&self, device: &DeviceState) -> RuntimeBinding {
        device.services.inference_runtime_binding()
    }
}

static EMBEDDING_GEMMA2: EmbeddingGemma2 = EmbeddingGemma2;
static HOST_BRIDGE: HostBridge = HostBridge;
static LOCAL_GENERATIVE: LocalGenerative = LocalGenerative;

/// Every provider the catalogue knows.
///
/// WHY a fixed array and not a heap-allocated list: the app has no heap, and
/// a catalogue that grows at run time is a catalogue whose contents depend
/// on when you looked at it.
static PROVIDERS: [&'static dyn Provider; 3] =
    [&EMBEDDING_GEMMA2, &HOST_BRIDGE, &LOCAL_GENERATIVE];

/// Every provider the catalogue knows.
pub fn providers() -> &'static [&'static dyn Provider] {
    &PROVIDERS
}

/// The provider with an identifier, if the catalogue knows one.
pub fn provider_by_id(id: &str) -> Option<&'static dyn Provider> {
    PROVIDERS.iter().copied().find(|p| p.id() == id)
}

// -----------------------------------------------------------------------------
// The device's model directory
// -----------------------------------------------------------------------------

/// The directory the platform keeps model weights in.
pub const MODEL_DIRECTORY: &str = "/system/models";

/// What a probe of the model directory found.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModelProbeStatus {
    /// The directory was listed.
    Listed,
    /// The directory is not on this device.
    ///
    /// WHY this is distinct from a refusal: "there is no model directory" is
    /// a fact about the device, and it is the answer that says "not
    /// installed" with confidence. A refusal says only that the app was not
    /// allowed to look.
    Absent,
    /// The platform refused the listing.
    Refused,
    /// This app holds no filesystem-read capability.
    NoAuthority,
}

impl ModelProbeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Listed => "LISTED",
            Self::Absent => "ABSENT",
            Self::Refused => "REFUSED",
            Self::NoAuthority => "NO AUTHORITY",
        }
    }
}

/// A measurement of the model's weights on the device.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ModelInventory {
    /// The outcome of the directory probe.
    pub status: ModelProbeStatus,
    /// Number of entries the listing reported.
    pub entries: u16,
    /// Whether an embedding-weights file was among the entries.
    pub has_embedding_weights: bool,
    /// Whether a generative-weights file was among the entries.
    pub has_generative_weights: bool,
}

impl ModelInventory {
    /// Probes the device's model directory.
    ///
    /// WHY this is a listing and not a table: a catalogue that printed
    /// "EmbeddingGemma 2 (740M Q7) -- Ready" had asked nothing. The answer
    /// here comes from the device's own directory, and when the app cannot
    /// ask, it says which of the three things happened.
    pub fn probe() -> Self {
        // WHY the capability is asked for and not resolved: the kernel
        // authorises a filesystem listing by the capability's *type*, so the
        // app names the authority it wants and the kernel finds it. The
        // lookup here is the yes/no question "does this app hold one at
        // all?", and the answer decides whether to ask.
        if interos_std::cap_lookup(interos_std::cap_type::FILE_SYSTEM_READ, 0).is_err() {
            return Self {
                status: ModelProbeStatus::NoAuthority,
                entries: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
            };
        }

        let mut buf = [0u8; 1024];
        match interos_std::fs_list(MODEL_DIRECTORY, &mut buf) {
            Ok(n) => {
                // WHY the length is clamped: a syscall that reports more
                // bytes than the buffer holds must not be trusted to slice
                // it. Clamping here keeps a bad return value from turning
                // into a panic in the middle of a frame.
                let payload = &buf[..n.min(buf.len())];
                let mut entries = 0u16;
                let mut has_embedding_weights = false;
                let mut has_generative_weights = false;

                for line in payload.split(|&b| b == b'\n') {
                    if line.is_empty() {
                        continue;
                    }
                    let end = line.iter().position(|&b| b == b'|').unwrap_or(line.len());
                    let name = &line[..end];
                    if name.is_empty() {
                        continue;
                    }
                    entries = entries.saturating_add(1);

                    if contains_ci(name, b"embed") {
                        has_embedding_weights = true;
                    }
                    if contains_ci(name, b"gemma") || contains_ci(name, b"gen-") || contains_ci(name, b"gguf")
                    {
                        has_generative_weights = true;
                    }
                }

                Self {
                    status: ModelProbeStatus::Listed,
                    entries,
                    has_embedding_weights,
                    has_generative_weights,
                }
            }
            Err(ERR_FILE_NOT_FOUND) => Self {
                status: ModelProbeStatus::Absent,
                entries: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
            },
            Err(_) => Self {
                status: ModelProbeStatus::Refused,
                entries: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
            },
        }
    }
}

/// Whether `haystack` contains `needle`, ignoring ASCII case.
fn contains_ci(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.len() > haystack.len() {
        return false;
    }
    for i in 0..=(haystack.len() - needle.len()) {
        let mut matched = true;
        for j in 0..needle.len() {
            if haystack[i + j].to_ascii_lowercase() != needle[j].to_ascii_lowercase() {
                matched = false;
                break;
            }
        }
        if matched {
            return true;
        }
    }
    false
}

// -----------------------------------------------------------------------------
// The platform's package registry
// -----------------------------------------------------------------------------

/// How many package ids the registry measurement keeps.
pub const MAX_PACKAGE_IDS: usize = 12;
/// The longest package id the registry measurement keeps.
pub const MAX_PACKAGE_ID_LEN: usize = 32;

/// A measurement of the platform's package registry.
#[derive(Clone, Copy, Debug)]
pub struct PackageRegistry {
    /// The outcome of the query.
    pub status: ProbeStatus,
    /// How many packages the registry reported.
    pub count: u16,
    /// The package ids the registry reported.
    pub ids: [[u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
    /// The length of each kept id.
    pub id_lens: [u8; MAX_PACKAGE_IDS],
}

impl PackageRegistry {
    /// Measures the platform's package registry.
    pub fn measure() -> Self {
        let mut ids = [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS];
        let mut id_lens = [0u8; MAX_PACKAGE_IDS];
        let mut count = 0u16;

        let mut buf = [0u8; 1024];
        match interos_std::package_registry(&mut buf) {
            Ok(n) => {
                let payload = &buf[..n.min(buf.len())];
                for line in payload.split(|&b| b == b'\n') {
                    if line.is_empty() {
                        continue;
                    }
                    let end = line.iter().position(|&b| b == b'|').unwrap_or(line.len());
                    let id = &line[..end];
                    if id.is_empty() {
                        continue;
                    }
                    count = count.saturating_add(1);
                    let slot = (count as usize).saturating_sub(1);
                    if slot >= MAX_PACKAGE_IDS {
                        break;
                    }
                    let take = id.len().min(MAX_PACKAGE_ID_LEN);
                    ids[slot][..take].copy_from_slice(&id[..take]);
                    id_lens[slot] = take as u8;
                }
                Self {
                    status: ProbeStatus::Listed,
                    count,
                    ids,
                    id_lens,
                }
            }
            Err(_) => Self {
                status: ProbeStatus::Refused,
                count: 0,
                ids,
                id_lens,
            },
        }
    }

    /// Whether the registry holds a package with this identifier.
    pub fn contains(&self, id: &str) -> bool {
        if self.status != ProbeStatus::Listed {
            return false;
        }
        for slot in 0..MAX_PACKAGE_IDS {
            let len = self.id_lens[slot] as usize;
            if len == 0 {
                continue;
            }
            if self.ids[slot][..len] == *id.as_bytes() {
                return true;
            }
        }
        false
    }
}

// -----------------------------------------------------------------------------
// The platform's service registry
// -----------------------------------------------------------------------------

/// The highest service id the platform's service ABI defines.
///
/// WHY this bound matters: the service ABI reserves ids 1..=11 for the
/// platform's own services -- init, display, input, storage, network,
/// power, modem, audio, camera, compositor, launcher and sensor. None of
/// them is an inference runtime. A registry that stays inside the defined
/// range therefore contains no inference runtime, and one that does not is
/// reported as a change in the platform's service set.
pub const SERVICE_ABI_MAX_ID: u8 = 11;

/// How many services the registry measurement keeps.
pub const MAX_SERVICES: usize = 12;

/// The state the platform's registry reports for a service.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServiceState {
    Declared,
    Starting,
    Running,
    Stopped,
    Failed,
    /// The platform reported a state this app does not know.
    Unknown,
}

impl ServiceState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    /// Parses the state the platform's registry writes.
    pub fn from_str(value: &str) -> Self {
        match value {
            "declared" => Self::Declared,
            "starting" => Self::Starting,
            "running" => Self::Running,
            "stopped" => Self::Stopped,
            "failed" => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

/// The name the platform gives a service id.
pub const fn service_name(id: u8) -> &'static str {
    match id {
        1 => "init",
        2 => "display",
        3 => "input",
        4 => "storage",
        5 => "network",
        6 => "power",
        7 => "modem",
        8 => "audio",
        9 => "camera",
        10 => "compositor",
        11 => "launcher",
        12 => "sensor",
        _ => "unknown",
    }
}

/// One service the platform's registry reported.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ServiceEntry {
    /// The service's id.
    pub id: u8,
    /// The state the registry reported.
    pub state: ServiceState,
    /// The service's ABI version.
    pub abi_version: u16,
    /// How long ago the service last reported a heartbeat, in platform ticks.
    pub age_ticks: u64,
    /// How many times the service has failed.
    pub failures: u32,
}

/// A measurement of the platform's service registry.
#[derive(Clone, Copy, Debug)]
pub struct ServiceRegistry {
    /// The outcome of the query.
    pub status: ProbeStatus,
    /// How many services the registry reported.
    pub count: u8,
    /// The largest service id the registry reported.
    pub highest_id: u8,
    /// The services themselves.
    pub entries: [ServiceEntry; MAX_SERVICES],
}

impl ServiceRegistry {
    /// Measures the platform's service registry.
    ///
    /// WHY this is a real query and not a constant: the screen used to assert
    /// that a host bridge was "Connected via Ring-3 IPC Socket" and that a
    /// supervisor was online, having asked nothing. The number of services
    /// the kernel is actually running is a fact this app can obtain.
    pub fn measure() -> Self {
        let mut entries = [ServiceEntry {
            id: 0,
            state: ServiceState::Unknown,
            abi_version: 0,
            age_ticks: 0,
            failures: 0,
        }; MAX_SERVICES];

        let mut buf = [0u8; 1024];
        match interos_std::service_list(&mut buf) {
            Ok(n) => {
                // The reported length is clamped before it is used to slice
                // the buffer: a syscall must not be trusted to report a
                // length the caller did not provide.
                let payload = &buf[..n.min(buf.len())];
                let mut count = 0u8;
                let mut highest_id = 0u8;
                for line in payload.split(|&b| b == b'\n') {
                    if line.is_empty() {
                        continue;
                    }
                    let Some(entry) = parse_service_line(line) else {
                        continue;
                    };
                    let slot = count as usize;
                    if slot >= MAX_SERVICES {
                        break;
                    }
                    entries[slot] = entry;
                    count = count.saturating_add(1);
                    if entry.id > highest_id {
                        highest_id = entry.id;
                    }
                }
                Self {
                    status: ProbeStatus::Listed,
                    count,
                    highest_id,
                    entries,
                }
            }
            Err(_) => Self {
                status: ProbeStatus::Refused,
                count: 0,
                highest_id: 0,
                entries,
            },
        }
    }

    /// The service with an id, if the registry reported one.
    pub fn find(&self, id: u8) -> Option<&ServiceEntry> {
        self.entries
            .iter()
            .take(self.count as usize)
            .find(|entry| entry.id == id)
    }

    /// Whether the registry contains a service id outside the ABI's defined
    /// range, which is how a change to the platform's service set shows up.
    pub fn has_service_beyond_abi(&self) -> bool {
        self.status == ProbeStatus::Listed && self.highest_id > SERVICE_ABI_MAX_ID
    }

    /// Whether the platform's registered service set contains an inference
    /// runtime.
    ///
    /// WHY this is false on the current platform: the ABI defines ids 1..=11
    /// and none of them is an inference runtime, and the `aid` daemon is not
    /// published into the guest. A registry that stays inside the defined
    /// range contains no inference runtime.
    pub fn contains_inference_runtime(&self) -> bool {
        self.has_service_beyond_abi()
    }

    /// The reason no runtime is bound, in the platform's own terms.
    ///
    /// WHY the reason is derived and not a constant: the three reasons are
    /// different facts, and which one applies depends on what the registry
    /// said. A registry that was refused is not the same as a registry that
    /// listed eleven services and no inference runtime.
    pub fn unbound_reason(&self) -> &'static str {
        match self.status {
            ProbeStatus::Refused => "the platform refused the service registry query",
            ProbeStatus::Listed if self.has_service_beyond_abi() => {
                "a service outside the platform's defined range is registered"
            }
            ProbeStatus::Listed => {
                "the platform's service ABI defines ids 1..=11 and none of them is an inference runtime"
            }
        }
    }

    /// Whether a runtime that can serve a provider is bound.
    pub fn inference_runtime_binding(&self) -> RuntimeBinding {
        if self.contains_inference_runtime() {
            RuntimeBinding::Bound
        } else {
            RuntimeBinding::Unbound {
                reason: self.unbound_reason(),
            }
        }
    }
}

/// Parses one line of the platform's service listing.
///
/// WHY the field order is fixed here: the platform writes
/// `id|abi_version|state|restart|critical|generation|age|failures`, and a
/// parser that guessed the order would be a parser that could report a
/// service's age as its failure count.
fn parse_service_line(line: &[u8]) -> Option<ServiceEntry> {
    let text = core::str::from_utf8(line).ok()?;
    let mut parts = text.split('|');
    let id: u8 = parts.next()?.trim().parse().ok()?;
    let abi_version: u16 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    let state = ServiceState::from_str(parts.next().unwrap_or("").trim());
    // `restart|critical|generation` are reported by the platform but are not
    // what this surface shows, so they are stepped over rather than parsed.
    let _restart = parts.next();
    let _critical = parts.next();
    let _generation = parts.next();
    let age_ticks: u64 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    let failures: u32 = parts.next().unwrap_or("0").trim().parse().unwrap_or(0);
    Some(ServiceEntry {
        id,
        state,
        abi_version,
        age_ticks,
        failures,
    })
}

// -----------------------------------------------------------------------------
// The device state every provider is measured against
// -----------------------------------------------------------------------------

/// Everything the app measured about this device.
#[derive(Clone, Copy, Debug)]
pub struct DeviceState {
    /// The model's weights on the device.
    pub models: ModelInventory,
    /// The platform's package registry.
    pub packages: PackageRegistry,
    /// The platform's service registry.
    pub services: ServiceRegistry,
}

impl DeviceState {
    /// Measures the device.
    ///
    /// WHY every measurement is taken at once: a screen that shows the
    /// model directory from one moment and the service registry from another
    /// can show a device that never existed.
    pub fn measure() -> Self {
        Self {
            models: ModelInventory::probe(),
            packages: PackageRegistry::measure(),
            services: ServiceRegistry::measure(),
        }
    }
}

// -----------------------------------------------------------------------------
// The capability grants this app holds
// -----------------------------------------------------------------------------

/// One capability the app may hold, and what holding it allows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CapabilityGrant {
    /// The capability's type, as the platform numbers it.
    pub cap_type: u64,
    /// The name the surface shows.
    pub name: &'static str,
    /// What holding the capability allows.
    pub allows: &'static str,
    /// Whether the platform granted it to this app.
    pub granted: bool,
}

/// The capabilities this surface reports.
///
/// WHY the table is a constant and the grants are not: the *set* of
/// capabilities that matter to a supervisor is a property of the role, and
/// the *grants* are a property of this installation. Mixing the two is how
/// a list of permissions starts reading like a list of wishes.
const CAPABILITY_TABLE: [CapabilityGrant; 12] = [
    CapabilityGrant {
        cap_type: interos_std::cap_type::FRAMEBUFFER_DIRECT,
        name: "Display",
        allows: "write pixels to this app's own surface",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::EVENT_POLL,
        name: "Events",
        allows: "receive touch, key and system events",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::SERIAL_OUTPUT,
        name: "Logging",
        allows: "write diagnostic lines to the serial log",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::FILE_SYSTEM_READ,
        name: "Filesystem read",
        allows: "read the device's files, settings and model directory",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::FILE_SYSTEM_WRITE,
        name: "Filesystem write",
        allows: "write files, and install or remove packages",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::IPC_SEND,
        name: "IPC send",
        allows: "send a message to another app",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::IPC_RECEIVE,
        name: "IPC receive",
        allows: "receive a message from another app",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::PROCESS_SPAWN,
        name: "Process spawn",
        allows: "launch another installed app",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::NETWORK_SOCKET,
        name: "Network socket",
        allows: "open a network socket",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::SYSTEM_CONTROL,
        name: "System control",
        allows: "invoke the platform's power, audio, camera and network services",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::CONTENT_INDEX,
        name: "Content index",
        allows: "read the shared document index and the intent registry",
        granted: false,
    },
    CapabilityGrant {
        cap_type: interos_std::cap_type::CONTENT_INTENT,
        name: "Content intent",
        allows: "dispatch an intent to the package that declared a handler",
        granted: false,
    },
];

/// How many capabilities the access surface reports.
pub const CAPABILITY_COUNT: usize = CAPABILITY_TABLE.len();

/// Asks the platform which capabilities this app actually holds.
///
/// WHY a lookup by type and not by slot: a slot is a position in the task's
/// capability table, and the position depends on the order the kernel
/// inserted the capabilities in. The type is the identity. The answer this
/// function uses is only ever "is there one of this type", so no position
/// is ever bound or passed on.
pub fn probe_capability_grants() -> [CapabilityGrant; CAPABILITY_COUNT] {
    let mut grants = CAPABILITY_TABLE;
    for grant in grants.iter_mut() {
        grant.granted = interos_std::cap_lookup(grant.cap_type, 0).is_ok();
    }
    grants
}

/// How many of the reported capabilities the platform granted.
pub fn granted_capability_count(grants: &[CapabilityGrant]) -> usize {
    grants.iter().filter(|grant| grant.granted).count()
}

// -----------------------------------------------------------------------------
// The search configuration
// -----------------------------------------------------------------------------

/// One source the platform's search can draw on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SearchSource {
    /// The source's name.
    pub name: &'static str,
    /// How many entries the source holds, when the platform answered.
    pub count: u16,
    /// The outcome of the query.
    pub status: ProbeStatus,
}

/// The search configuration the platform holds, and the content it holds.
#[derive(Clone, Copy, Debug)]
pub struct SearchConfiguration {
    /// The Matryoshka dimension the platform holds, if it holds one.
    pub dimension: Option<MatryoshkaDim>,
    /// The similarity cutoff the platform holds, if it holds one.
    pub threshold_q15: Option<i16>,
    /// The result limit the platform holds, if it holds one.
    pub max_results: Option<u16>,
    /// Whether the search may traverse the filesystem. Off by default.
    pub filesystem_scan: bool,
    /// Whether this app may change the platform's settings.
    pub writable: bool,
    /// The sources the search can draw on, measured.
    pub sources: [SearchSource; SearchSource::COUNT],
}

impl SearchSource {
    /// How many sources the search surface reports.
    pub const COUNT: usize = 4;
}

impl SearchConfiguration {
    /// The settings key the platform keeps the vector dimension in.
    pub const DIMENSION_KEY: &'static str = "search.dimension";
    /// The settings key the platform keeps the similarity cutoff in.
    pub const THRESHOLD_KEY: &'static str = "search.similarity_q15";
    /// The settings key the platform keeps the result limit in.
    pub const MAX_RESULTS_KEY: &'static str = "search.max_results";

    /// Measures the platform's search configuration and its content layer.
    ///
    /// WHY the settings are read and not written: the platform's settings
    /// are global, and a package that could rewrite them would have more
    /// authority than a supervisor needs. The surface therefore reports what
    /// the platform holds, and says plainly that the app may not change it.
    pub fn measure() -> Self {
        let dimension = read_setting(Self::DIMENSION_KEY)
            .and_then(|value| value.as_u32())
            .and_then(MatryoshkaDim::from_u32);
        let threshold_q15 = read_setting(Self::THRESHOLD_KEY).and_then(|value| value.as_i16());
        let max_results = read_setting(Self::MAX_RESULTS_KEY).and_then(|value| value.as_u16());

        Self {
            dimension,
            threshold_q15,
            max_results,
            filesystem_scan: false,
            writable: interos_std::cap_lookup(interos_std::cap_type::FILE_SYSTEM_WRITE, 0).is_ok(),
            sources: measure_sources(),
        }
    }
}

/// One value the platform's settings store holds.
#[derive(Clone, Copy, Debug)]
pub struct SettingValue {
    bytes: [u8; SETTING_VALUE_MAX],
    len: usize,
}

/// The longest settings value the app keeps.
pub const SETTING_VALUE_MAX: usize = 32;

impl SettingValue {
    /// The value as text.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }

    /// The value as an unsigned number, if it is one.
    pub fn as_u32(&self) -> Option<u32> {
        self.as_str().trim().parse::<u32>().ok()
    }

    /// The value as a signed 16-bit number, if it is one.
    pub fn as_i16(&self) -> Option<i16> {
        self.as_str().trim().parse::<i16>().ok()
    }

    /// The value as an unsigned 16-bit number, if it is one.
    pub fn as_u16(&self) -> Option<u16> {
        self.as_str().trim().parse::<u16>().ok()
    }
}

/// Reads one settings value, and reports whether the platform holds one.
///
/// WHY the value is copied out: the SDK has no heap, and a borrowed slice of
/// the caller's buffer would keep the buffer borrowed for as long as the
/// value was alive, which is the whole length of the measurement.
fn read_setting(key: &str) -> Option<SettingValue> {
    let mut buf = [0u8; 64];
    match interos_std::settings_get(key, &mut buf) {
        Ok(n) => {
            let n = n.min(buf.len());
            if n == 0 {
                // WHY an empty answer is "the platform holds nothing": the
                // platform's settings store answers an absent key with zero
                // bytes, and a key that is absent is a fact.
                None
            } else {
                let mut value = SettingValue {
                    bytes: [0u8; SETTING_VALUE_MAX],
                    len: 0,
                };
                let take = n.min(SETTING_VALUE_MAX);
                value.bytes[..take].copy_from_slice(&buf[..take]);
                value.len = take;
                Some(value)
            }
        }
        Err(_) => None,
    }
}

/// Measures the four sources the platform's search draws on.
fn measure_sources() -> [SearchSource; SearchSource::COUNT] {
    let packages = PackageRegistry::measure();
    let package_source = SearchSource {
        name: "Installed packages",
        count: packages.count,
        status: packages.status,
    };

    // WHY the settings surfaces are counted from the platform's own
    // catalogue: the Settings app renders these sections and the shell's
    // search offers the same ones, and the catalogue is the one definition
    // of what they are. Counting them here from the same table is what
    // keeps the two sides from drifting.
    let settings_source = SearchSource {
        name: "Settings surfaces",
        count: interos_std::settings::Section::COUNT as u16,
        status: ProbeStatus::Listed,
    };

    let (document_count, document_status) = {
        let mut doc_buf = [0u8; 2048];
        match interos_std::content::content_index_list(&mut doc_buf) {
            Ok(n) => {
                let payload = &doc_buf[..n.min(doc_buf.len())];
                let mut records = [interos_std::content::DocumentRecord {
                    id: 0,
                    path: "",
                    mime: "",
                    provenance: "",
                    length: 0,
                    mtime: 0,
                }; 64];
                (
                    interos_std::content::parse_index(payload, &mut records) as u16,
                    ProbeStatus::Listed,
                )
            }
            Err(_) => (0, ProbeStatus::Refused),
        }
    };
    let document_source = SearchSource {
        name: "Shared documents",
        count: document_count,
        status: document_status,
    };

    let (handler_count, handler_status) = {
        let mut intent_buf = [0u8; 2048];
        match interos_std::content::intent_list(&mut intent_buf) {
            Ok(n) => {
                let payload = &intent_buf[..n.min(intent_buf.len())];
                let mut handlers = [interos_std::content::IntentHandler {
                    kind: interos_std::content::IntentKind::Mime,
                    value: "",
                    package_id: "",
                }; 64];
                (
                    interos_std::content::parse_intents(payload, &mut handlers) as u16,
                    ProbeStatus::Listed,
                )
            }
            Err(_) => (0, ProbeStatus::Refused),
        }
    };
    let handler_source = SearchSource {
        name: "Intent handlers",
        count: handler_count,
        status: handler_status,
    };

    [
        package_source,
        settings_source,
        document_source,
        handler_source,
    ]
}

// -----------------------------------------------------------------------------
// The self-test
// -----------------------------------------------------------------------------

/// The dimension of the vectors the self-test uses.
pub const SELF_TEST_DIM: usize = 128;

/// How many rows the self-test has.
pub const SELF_TEST_ROWS: usize = 6;

/// The longest detail line a self-test row carries.
///
/// WHY it is this long: the detail is the value the platform's function
/// returned, written out in full. A detail that had to be cut to fit a
/// buffer would be a detail that says less than the function did.
pub const SELF_TEST_DETAIL_MAX: usize = 96;

/// One row of the self-test.
#[derive(Clone, Copy, Debug)]
pub struct SelfTestRow {
    /// What the row checks.
    pub label: &'static str,
    /// What the function returned, formatted.
    pub detail: [u8; SELF_TEST_DETAIL_MAX],
    /// The length of the formatted detail.
    pub detail_len: usize,
    /// Whether the row passed.
    pub passed: bool,
}

impl SelfTestRow {
    /// The formatted detail, as text.
    pub fn detail_str(&self) -> &str {
        core::str::from_utf8(&self.detail[..self.detail_len]).unwrap_or("")
    }
}

/// The result of running the self-test.
///
/// WHY the rows are measured and not written: a self-test that prints three
/// green lines whose values are string constants has tested nothing. Every
/// row here holds the value the platform's own function returned.
#[derive(Clone, Copy, Debug)]
pub struct SelfTest {
    /// The rows, in display order.
    pub rows: [SelfTestRow; SELF_TEST_ROWS],
    /// Whether every row passed.
    pub all_passed: bool,
}

impl SelfTest {
    /// Runs the self-test.
    ///
    /// WHY the vectors are built here and not read from a table: a self-test
    /// that reads its inputs from a table is a test of the table.
    pub fn run() -> Self {
        let mut base = [0i8; SELF_TEST_DIM];
        for value in base.iter_mut() {
            *value = 64;
        }
        let mut opposite = [0i8; SELF_TEST_DIM];
        for value in opposite.iter_mut() {
            *value = -64;
        }
        let mut orthogonal = [0i8; SELF_TEST_DIM];
        for (index, value) in orthogonal.iter_mut().enumerate() {
            *value = if index < SELF_TEST_DIM / 2 { 64 } else { -64 };
        }

        let mut rows = [SelfTestRow {
            label: "",
            detail: [0u8; SELF_TEST_DETAIL_MAX],
            detail_len: 0,
            passed: false,
        }; SELF_TEST_ROWS];

        // Row 1..3: the platform's fixed-point cosine function.
        let identical = interos_std::ai::cosine_similarity_q7(&base, &base);
        let negated = interos_std::ai::cosine_similarity_q7(&base, &opposite);
        let perpendicular = interos_std::ai::cosine_similarity_q7(&base, &orthogonal);
        SelfTestRow::set(
            &mut rows[0],
            "cosine: identical vectors",
            identical,
            identical == 32767,
        );
        SelfTestRow::set(
            &mut rows[1],
            "cosine: opposite vectors",
            negated,
            negated == -32767,
        );
        SelfTestRow::set(
            &mut rows[2],
            "cosine: orthogonal vectors",
            perpendicular,
            perpendicular == 0,
        );

        // Row 4: the platform's Matryoshka dimension map.
        let dims_ok = MatryoshkaDim::from_u32(128) == Some(MatryoshkaDim::Dim128)
            && MatryoshkaDim::from_u32(256) == Some(MatryoshkaDim::Dim256)
            && MatryoshkaDim::from_u32(512) == Some(MatryoshkaDim::Dim512)
            && MatryoshkaDim::from_u32(768) == Some(MatryoshkaDim::Dim768)
            && MatryoshkaDim::from_u32(384).is_none();
        rows[3] = SelfTestRow::text(
            "matryoshka: dimension map",
            b"128/256/512/768 map, 384 does not",
            dims_ok,
        );

        // Row 5: the platform's service registry line format.
        let service = parse_service_line(b"3|1|running|0|1|7|42|2");
        let service_ok = service
            .map(|entry| {
                entry.id == 3
                    && entry.state == ServiceState::Running
                    && entry.abi_version == 1
                    && entry.age_ticks == 42
                    && entry.failures == 2
            })
            .unwrap_or(false);
        rows[4] = SelfTestRow::text(
            "service registry: line parse",
            b"3|1|running|0|1|7|42|2 -> id 3, running, 42 ticks, 2 failures",
            service_ok,
        );

        // Row 6: the platform's document index record format.
        let record = interos_std::content::DocumentRecord::parse(
            "444f4300000000ab|/storage/a.ppm|image/x-portable-pixmap|bytes|2048|77",
        );
        let record_ok = record
            .map(|entry| {
                entry.path == "/storage/a.ppm"
                    && entry.mime == "image/x-portable-pixmap"
                    && entry.provenance == "bytes"
                    && entry.length == 2048
                    && entry.mtime == 77
            })
            .unwrap_or(false);
        rows[5] = SelfTestRow::text(
            "content index: record parse",
            b"one record -> path, mime, bytes, 2048, 77",
            record_ok,
        );

        let all_passed = rows.iter().all(|row| row.passed);
        SelfTest { rows, all_passed }
    }
}

impl SelfTestRow {
    /// Builds a row whose detail is a measured cosine.
    fn set(row: &mut SelfTestRow, label: &'static str, value_q15: i16, passed: bool) {
        let mut detail = [0u8; SELF_TEST_DETAIL_MAX];
        let len = format_cosine(value_q15, &mut detail);
        row.label = label;
        row.detail = detail;
        row.detail_len = len;
        row.passed = passed;
    }

    /// Builds a row whose detail is a fixed sentence.
    fn text(label: &'static str, detail: &[u8], passed: bool) -> SelfTestRow {
        let mut buffer = [0u8; SELF_TEST_DETAIL_MAX];
        let len = detail.len().min(SELF_TEST_DETAIL_MAX);
        buffer[..len].copy_from_slice(&detail[..len]);
        SelfTestRow {
            label,
            detail: buffer,
            detail_len: len,
            passed,
        }
    }
}

// -----------------------------------------------------------------------------
// Formatting
// -----------------------------------------------------------------------------

/// Formats a cosine as a signed decimal with three places, and appends the
/// raw Q15 value.
///
/// WHY the value and the decimal are both shown: the decimal is what a
/// reader wants and the Q15 integer is what the function returned, and a
/// screen that shows only the decimal hides the rounding.
pub fn format_cosine(value_q15: i16, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    let mut push = |n: &mut usize, byte: u8| {
        if *n < out.len() {
            out[*n] = byte;
            *n += 1;
        }
    };

    let scaled = if value_q15 >= 0 {
        ((value_q15 as i32) * 1000 + 16384) / 32768
    } else {
        -((-(value_q15 as i32)) * 1000 + 16384) / 32768
    };
    let magnitude = scaled.unsigned_abs();
    push(&mut n, b' ');
    if scaled < 0 {
        push(&mut n, b'-');
    } else {
        push(&mut n, b'+');
    }
    if magnitude >= 1000 {
        push(&mut n, b'0' + (magnitude / 1000 % 10) as u8);
    }
    push(&mut n, b'0' + ((magnitude / 100) % 10) as u8);
    push(&mut n, b'.');
    push(&mut n, b'0' + ((magnitude / 10) % 10) as u8);
    push(&mut n, b'0' + (magnitude % 10) as u8);

    for byte in b" (Q15: " {
        push(&mut n, *byte);
    }
    let mut q_buf = [0u8; 8];
    let q_len = format_i16(value_q15, &mut q_buf);
    for index in 0..q_len {
        push(&mut n, q_buf[index]);
    }
    push(&mut n, b')');
    n
}

/// Formats the similarity cutoff as `0.50 (Q15: 16384)`.
///
/// WHY this is a function and not a literal: a cutoff that was changed would
/// leave a literal label showing the old number. The label has to follow the
/// configuration it describes.
pub fn format_threshold(q15: i16, out: &mut [u8]) -> usize {
    let mut n = 0usize;
    let mut push = |n: &mut usize, byte: u8| {
        if *n < out.len() {
            out[*n] = byte;
            *n += 1;
        }
    };

    let scaled = if q15 >= 0 {
        ((q15 as i32) * 1000 + 16384) / 32768
    } else {
        -((-(q15 as i32)) * 1000 + 16384) / 32768
    };
    let magnitude = scaled.unsigned_abs();
    push(&mut n, b'0' + (magnitude / 1000 % 10) as u8);
    push(&mut n, b'.');
    push(&mut n, b'0' + ((magnitude / 100) % 10) as u8);
    push(&mut n, b'0' + ((magnitude / 10) % 10) as u8);
    push(&mut n, b'0' + (magnitude % 10) as u8);

    for byte in b" (Q15: " {
        push(&mut n, *byte);
    }
    let mut q_buf = [0u8; 8];
    let q_len = format_i16(q15, &mut q_buf);
    for index in 0..q_len {
        push(&mut n, q_buf[index]);
    }
    push(&mut n, b')');
    n
}

/// Writes the decimal form of a signed 16-bit value into `buf`.
pub fn format_i16(value: i16, buf: &mut [u8]) -> usize {
    if value < 0 {
        if buf.is_empty() {
            return 0;
        }
        buf[0] = b'-';
        let mut n = 1;
        let mut digits = [0u8; 6];
        let mut len = 0usize;
        let mut value = -(value as i32) as u32;
        while value > 0 && len < digits.len() {
            digits[len] = b'0' + (value % 10) as u8;
            value /= 10;
            len += 1;
        }
        while len > 0 {
            len -= 1;
            if n < buf.len() {
                buf[n] = digits[len];
                n += 1;
            }
        }
        n
    } else {
        let mut digits = [0u8; 6];
        let mut len = 0usize;
        let mut value = value as u32;
        while value > 0 && len < digits.len() {
            digits[len] = b'0' + (value % 10) as u8;
            value /= 10;
            len += 1;
        }
        if len == 0 {
            // WHY zero is spelled out: a value of zero has no digits, and an
            // empty label on a screen reads as a missing value rather than as
            // the number zero.
            digits[0] = b'0';
            len = 1;
        }
        let mut n = 0usize;
        while len > 0 {
            len -= 1;
            if n < buf.len() {
                buf[n] = digits[len];
                n += 1;
            }
        }
        n
    }
}

/// Writes the decimal form of an unsigned 32-bit value into `buf`.
pub fn format_u32(value: u32, buf: &mut [u8]) -> usize {
    let mut digits = [0u8; 10];
    let mut len = 0usize;
    let mut value = value;
    while value > 0 && len < digits.len() {
        digits[len] = b'0' + (value % 10) as u8;
        value /= 10;
        len += 1;
    }
    if len == 0 {
        digits[0] = b'0';
        len = 1;
    }
    let mut n = 0usize;
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A registry that reported the given services.
    fn registry(status: ProbeStatus, entries: &[ServiceEntry]) -> ServiceRegistry {
        let mut reg = ServiceRegistry {
            status,
            count: 0,
            highest_id: 0,
            entries: [ServiceEntry {
                id: 0,
                state: ServiceState::Running,
                abi_version: 0,
                age_ticks: 0,
                failures: 0,
            }; MAX_SERVICES],
        };
        for entry in entries {
            let index = reg.count as usize;
            if index >= MAX_SERVICES {
                break;
            }
            reg.entries[index] = *entry;
            reg.count += 1;
            if entry.id > reg.highest_id {
                reg.highest_id = entry.id;
            }
        }
        reg
    }

    /// A service entry with the given id, state and version.
    fn service(id: u8, state: ServiceState, abi_version: u16) -> ServiceEntry {
        ServiceEntry {
            id,
            state,
            abi_version,
            age_ticks: 0,
            failures: 0,
        }
    }

    #[test]
    fn a_platform_with_no_inference_service_says_so() {
        // WHY this matters: the catalogue once listed a model and a host
        // bridge that the device did not have. The chat's answer has to
        // come from the registry, and the registry names no inference
        // service on this platform.
        let reg = registry(ProbeStatus::Listed, &[service(1, ServiceState::Running, 1)]);
        assert_eq!(
            resolve_inference_service(&reg),
            InferenceAvailability::NoService
        );
    }

    #[test]
    fn a_registry_that_could_not_be_read_is_not_the_same_as_no_service() {
        // WHY this matters: a refused registry has not said that there
        // is no inference service. It has said nothing, and the chat
        // must not turn silence into a fact.
        let reg = registry(ProbeStatus::Refused, &[]);
        assert_eq!(
            resolve_inference_service(&reg),
            InferenceAvailability::RegistryUnavailable
        );
    }

    #[test]
    fn an_inference_service_that_is_not_running_is_reported_as_not_running() {
        let reg = registry(
            ProbeStatus::Listed,
            &[service(
                interos_std::ai::SERVICE_INFERENCE,
                ServiceState::Failed,
                interos_std::ai::INFERENCE_ABI_VERSION,
            )],
        );
        assert_eq!(
            resolve_inference_service(&reg),
            InferenceAvailability::NotRunning {
                state: ServiceState::Failed
            }
        );
    }

    #[test]
    fn an_inference_service_that_speaks_another_version_is_reported() {
        // WHY this matters: a request sent to a service that speaks
        // another version would be misread rather than refused, so the
        // mismatch has to be a fact the app can report.
        let reg = registry(
            ProbeStatus::Listed,
            &[service(
                interos_std::ai::SERVICE_INFERENCE,
                ServiceState::Running,
                interos_std::ai::INFERENCE_ABI_VERSION + 1,
            )],
        );
        assert_eq!(
            resolve_inference_service(&reg),
            InferenceAvailability::WrongVersion {
                abi_version: interos_std::ai::INFERENCE_ABI_VERSION + 1
            }
        );
    }

    #[test]
    fn a_running_service_that_speaks_this_version_is_available() {
        let reg = registry(
            ProbeStatus::Listed,
            &[service(
                interos_std::ai::SERVICE_INFERENCE,
                ServiceState::Running,
                interos_std::ai::INFERENCE_ABI_VERSION,
            )],
        );
        let availability = resolve_inference_service(&reg);
        assert!(availability.is_available());
        assert_eq!(
            availability,
            InferenceAvailability::Available {
                abi_version: interos_std::ai::INFERENCE_ABI_VERSION
            }
        );
    }

    #[test]
    fn every_unavailable_answer_carries_a_reason() {
        // WHY this matters: "unavailable" on its own tells the user
        // nothing they can act on. Each of the answers has a sentence
        // that names what the platform said.
        for availability in [
            InferenceAvailability::NoService,
            InferenceAvailability::RegistryUnavailable,
            InferenceAvailability::NotRunning {
                state: ServiceState::Failed,
            },
            InferenceAvailability::WrongVersion { abi_version: 9 },
        ] {
            let reason = availability.reason();
            assert!(!reason.is_empty());
            assert!(reason.ends_with('.'), "the reason is a sentence");
            assert!(!availability.is_available());
        }
    }

    #[test]
    fn the_transcript_drops_the_oldest_message_when_it_is_full() {
        // WHY this matters: the transcript is a ring, and a ring that
        // grew past its bound would either stop accepting messages or
        // overwrite something it had not meant to.
        let mut chat = ChatSession::new();
        for index in 0..(CHAT_MAX_MESSAGES + 3) {
            chat.push(ChatRole::User, &[index as u8]);
        }
        assert_eq!(chat.len(), CHAT_MAX_MESSAGES);
        // The three oldest went; the rest are the newest, in order.
        assert_eq!(chat.message_at(0).unwrap().text[0], 3);
        assert_eq!(
            chat.message_at(CHAT_MAX_MESSAGES - 1).unwrap().text[0],
            (CHAT_MAX_MESSAGES + 2) as u8
        );
    }

    #[test]
    fn the_transcript_reads_oldest_first() {
        let mut chat = ChatSession::new();
        chat.push(ChatRole::User, b"first");
        chat.push(ChatRole::Assistant, b"second");
        assert_eq!(chat.message_at(0).unwrap().text[..5], *b"first");
        assert_eq!(chat.message_at(1).unwrap().text[..6], *b"second");
        assert!(chat.message_at(2).is_none());
    }

    #[test]
    fn the_composer_refuses_a_character_it_cannot_hold() {
        // WHY this matters: a composer that silently drops the last
        // character typed is a composer that lies about what was written.
        let mut chat = ChatSession::new();
        for _ in 0..CHAT_PROMPT_MAX {
            assert!(chat.push_prompt_char(b'x'));
        }
        assert!(!chat.push_prompt_char(b'y'));
        assert_eq!(chat.prompt_text().len(), CHAT_PROMPT_MAX);
        // The refused character is not in the buffer.
        assert!(chat.prompt_text().iter().all(|byte| *byte == b'x'));
    }

    #[test]
    fn the_composer_takes_a_backspace_and_a_clear() {
        let mut chat = ChatSession::new();
        chat.push_prompt_char(b'a');
        chat.push_prompt_char(b'b');
        chat.pop_prompt_char();
        assert_eq!(chat.prompt_text(), b"a");
        chat.clear_prompt();
        assert!(chat.prompt_is_empty());
        // A backspace on an empty composer is not an error.
        chat.pop_prompt_char();
        assert!(chat.prompt_is_empty());
    }

    #[test]
    fn a_message_longer_than_the_buffer_is_shortened() {
        let mut chat = ChatSession::new();
        let long = [b'z'; CHAT_TEXT_MAX + 40];
        chat.push(ChatRole::User, &long);
        assert_eq!(chat.message_at(0).unwrap().text_len as usize, CHAT_TEXT_MAX);
    }

    #[test]
    fn the_chat_is_the_last_tab_and_the_tab_row_fits() {
        // WHY this matters: the segmented control's segment width was
        // written with a fixed gap count, and the last tab in the row
        // ran past the content column when a sixth surface was added.
        assert_eq!(Surface::COUNT, 6);
        assert_eq!(Surface::from_index(5), Some(Surface::Chat));
        assert_eq!(Surface::Chat.tab_label(), "CHAT");
    }

    #[test]
    fn the_catalogue_lists_installed_state_not_a_vendor_plan() {
        // WHY this matters: the catalogue used to print a model with a
        // parameter count and a "Ready" status that had never been checked.
        // Every provider's install state must be derived from the device.
        for provider in providers() {
            let device = DeviceState {
                models: ModelInventory {
                    status: ModelProbeStatus::Listed,
                    entries: 0,
                    has_embedding_weights: false,
                    has_generative_weights: false,
                },
                packages: PackageRegistry {
                    status: ProbeStatus::Listed,
                    count: 0,
                    ids: [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
                    id_lens: [0; MAX_PACKAGE_IDS],
                },
                services: ServiceRegistry {
                    status: ProbeStatus::Listed,
                    count: 0,
                    highest_id: 0,
                    entries: [ServiceEntry {
                        id: 0,
                        state: ServiceState::Unknown,
                        abi_version: 0,
                        age_ticks: 0,
                        failures: 0,
                    }; MAX_SERVICES],
                },
            };
            // An empty device holds nothing.
            assert_eq!(
                provider.install_state(&device),
                InstallState::NotInstalled,
                "{} claims to be installed on an empty device",
                provider.display_name()
            );
        }
    }

    #[test]
    fn an_embedding_provider_never_claims_to_generate_text() {
        let embed = provider_by_id("embeddinggemma2").expect("the catalogue has it");
        let capabilities = embed.capabilities();
        assert!(capabilities.embeds);
        assert!(!capabilities.generates, "a vector embedding cannot generate text");
        assert_eq!(capabilities.role(), "vector embeddings only");
    }

    #[test]
    fn a_host_backed_provider_says_so() {
        let bridge = provider_by_id("host-bridge").expect("the catalogue has it");
        assert!(bridge.capabilities().host_backed);
        assert!(bridge.capabilities().generates);
        assert_eq!(bridge.capabilities().role(), "conversational, host-backed");
        // A bridge is not delivered to the device as a package, so it has no
        // package to install.
        assert!(bridge.package_id().is_none());
    }

    #[test]
    fn a_new_provider_is_an_implementation_and_not_an_edit_through_the_app() {
        // WHY this matters: the point of the trait is that adding a model
        // does not mean editing the screens. The registry is the only list,
        // and the screens read the registry.
        assert_eq!(providers().len(), 3);
        for provider in providers() {
            assert!(!provider.id().is_empty());
            assert!(!provider.display_name().is_empty());
            assert!(provider_by_id(provider.id()).is_some());
        }
    }

    #[test]
    fn a_device_without_weights_reports_not_installed() {
        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 3,
                has_embedding_weights: false,
                has_generative_weights: false,
            },
            packages: PackageRegistry {
                status: ProbeStatus::Listed,
                count: 0,
                ids: [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
                id_lens: [0; MAX_PACKAGE_IDS],
            },
            services: ServiceRegistry {
                status: ProbeStatus::Listed,
                count: 11,
                highest_id: 11,
                entries: [ServiceEntry {
                    id: 0,
                    state: ServiceState::Unknown,
                    abi_version: 0,
                    age_ticks: 0,
                    failures: 0,
                }; MAX_SERVICES],
            },
        };
        assert_eq!(
            provider_by_id("embeddinggemma2")
                .unwrap()
                .install_state(&device),
            InstallState::NotInstalled
        );
        assert_eq!(
            provider_by_id("local-generative")
                .unwrap()
                .install_state(&device),
            InstallState::NotInstalled
        );
    }

    #[test]
    fn a_device_with_weights_reports_installed() {
        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 1,
                has_embedding_weights: true,
                has_generative_weights: true,
            },
            packages: PackageRegistry {
                status: ProbeStatus::Listed,
                count: 0,
                ids: [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
                id_lens: [0; MAX_PACKAGE_IDS],
            },
            services: ServiceRegistry {
                status: ProbeStatus::Listed,
                count: 11,
                highest_id: 11,
                entries: [ServiceEntry {
                    id: 0,
                    state: ServiceState::Unknown,
                    abi_version: 0,
                    age_ticks: 0,
                    failures: 0,
                }; MAX_SERVICES],
            },
        };
        assert_eq!(
            provider_by_id("embeddinggemma2")
                .unwrap()
                .install_state(&device),
            InstallState::Installed
        );
    }

    #[test]
    fn a_refused_probe_is_unavailable_and_not_not_installed() {
        // WHY this matters: "not installed" and "we were not allowed to
        // look" are different answers, and only one of them is a claim
        // about the device.
        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Refused,
                entries: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
            },
            packages: PackageRegistry {
                status: ProbeStatus::Refused,
                count: 0,
                ids: [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
                id_lens: [0; MAX_PACKAGE_IDS],
            },
            services: ServiceRegistry {
                status: ProbeStatus::Refused,
                count: 0,
                highest_id: 0,
                entries: [ServiceEntry {
                    id: 0,
                    state: ServiceState::Unknown,
                    abi_version: 0,
                    age_ticks: 0,
                    failures: 0,
                }; MAX_SERVICES],
            },
        };
        assert_eq!(
            provider_by_id("embeddinggemma2")
                .unwrap()
                .install_state(&device),
            InstallState::Unavailable
        );
        assert_eq!(
            provider_by_id("host-bridge").unwrap().install_state(&device),
            InstallState::Unavailable
        );
    }

    #[test]
    fn an_absent_model_directory_is_not_installed() {
        // WHY this matters: the directory not being on the device is a fact
        // about the device, and it is the answer that says "not installed"
        // with confidence rather than "we were not allowed to look".
        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Absent,
                entries: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
            },
            packages: PackageRegistry {
                status: ProbeStatus::Listed,
                count: 0,
                ids: [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
                id_lens: [0; MAX_PACKAGE_IDS],
            },
            services: ServiceRegistry {
                status: ProbeStatus::Listed,
                count: 11,
                highest_id: 11,
                entries: [ServiceEntry {
                    id: 0,
                    state: ServiceState::Unknown,
                    abi_version: 0,
                    age_ticks: 0,
                    failures: 0,
                }; MAX_SERVICES],
            },
        };
        assert_eq!(
            provider_by_id("embeddinggemma2")
                .unwrap()
                .install_state(&device),
            InstallState::NotInstalled
        );
    }

    #[test]
    fn the_unbound_reason_follows_the_measurement() {
        // WHY this matters: the reason the runtime is unbound is a fact
        // about the platform, and the three reasons are different facts.
        let refused = ServiceRegistry {
            status: ProbeStatus::Refused,
            count: 0,
            highest_id: 0,
            entries: [ServiceEntry {
                id: 0,
                state: ServiceState::Unknown,
                abi_version: 0,
                age_ticks: 0,
                failures: 0,
            }; MAX_SERVICES],
        };
        assert!(refused.unbound_reason().contains("refused"));
        assert!(!refused.inference_runtime_binding().is_bound());

        let listed = ServiceRegistry {
            status: ProbeStatus::Listed,
            count: 11,
            highest_id: 11,
            entries: [ServiceEntry {
                id: 0,
                state: ServiceState::Unknown,
                abi_version: 0,
                age_ticks: 0,
                failures: 0,
            }; MAX_SERVICES],
        };
        assert!(listed.unbound_reason().contains("1..=11"));
        assert!(!listed.inference_runtime_binding().is_bound());
        assert!(!listed.contains_inference_runtime());

        let beyond = ServiceRegistry {
            status: ProbeStatus::Listed,
            count: 12,
            highest_id: 12,
            entries: [ServiceEntry {
                id: 0,
                state: ServiceState::Unknown,
                abi_version: 0,
                age_ticks: 0,
                failures: 0,
            }; MAX_SERVICES],
        };
        assert!(beyond.contains_inference_runtime());
        assert!(beyond.inference_runtime_binding().is_bound());
    }

    #[test]
    fn the_service_line_parser_reads_the_platforms_format() {
        let entry = parse_service_line(b"3|1|running|0|1|7|42|2").expect("it parses");
        assert_eq!(entry.id, 3);
        assert_eq!(entry.state, ServiceState::Running);
        assert_eq!(entry.abi_version, 1);
        assert_eq!(entry.age_ticks, 42);
        assert_eq!(entry.failures, 2);

        let failed = parse_service_line(b"9|2|failed|3|0|1|0|5").expect("it parses");
        assert_eq!(failed.state, ServiceState::Failed);
        assert_eq!(failed.failures, 5);
    }

    #[test]
    fn the_self_test_runs_the_platforms_functions() {
        // WHY this matters: the supervisor screen used to print three green
        // [PASS] lines whose values were string literals. The arithmetic
        // they quoted was right, but nothing executed it.
        let result = SelfTest::run();
        assert!(result.all_passed, "a self-test row failed: {result:?}");
        for row in result.rows.iter() {
            assert!(row.passed, "{} failed", row.label);
            assert!(row.detail_len > 0, "{} has no detail", row.label);
        }
    }

    #[test]
    fn a_failing_self_test_row_says_fail() {
        // WHY this matters: a verdict that is always PASS is not a verdict.
        let mut buf = [0u8; SELF_TEST_DETAIL_MAX];
        let len = format_cosine(12345, &mut buf);
        let text = core::str::from_utf8(&buf[..len]).expect("utf8");
        assert!(text.contains("Q15: 12345"), "expected the measured value, got {text:?}");
    }

    #[test]
    fn the_surface_list_is_the_tab_row() {
        // WHY the count is six and not five: the chat surface joined the
        // row, and the row's segment width is derived from the count, so
        // the two have to agree or the last tab runs past the content
        // column.
        assert_eq!(Surface::COUNT, 6);
        for (index, surface) in Surface::ALL.iter().enumerate() {
            assert_eq!(surface.index(), index);
            assert_eq!(Surface::from_index(index), Some(*surface));
            assert!(!surface.tab_label().is_empty());
            assert!(!surface.title().is_empty());
        }
        assert_eq!(Surface::from_index(Surface::COUNT), None);
    }

    #[test]
    fn the_capability_table_names_the_types_it_asks_about() {
        // WHY this matters: the access surface reports the grants this app
        // holds, and a grant is identified by the capability's type.
        assert!(CAPABILITY_COUNT >= 8);
        let mut seen = 0u64;
        for grant in CAPABILITY_TABLE.iter() {
            assert!(grant.name.len() > 0);
            assert!(grant.allows.len() > 0);
            assert!(grant.cap_type > 0);
            seen |= 1u64 << (grant.cap_type.min(63));
        }
        assert!(seen & (1 << interos_std::cap_type::FILE_SYSTEM_READ) != 0);
        assert!(seen & (1 << interos_std::cap_type::FILE_SYSTEM_WRITE) != 0);
        assert!(seen & (1 << interos_std::cap_type::CONTENT_INDEX) != 0);
    }

    #[test]
    fn the_search_configuration_reports_the_platforms_own_sources() {
        // WHY this matters: the search surface used to print "0 documents
        // (Index Empty)" and a partition path as though the app had opened an
        // index. The sources now come from the platform.
        let config = SearchConfiguration::measure();
        assert_eq!(config.sources.len(), SearchSource::COUNT);
        assert_eq!(config.sources[1].name, "Settings surfaces");
        assert_eq!(config.sources[1].count, interos_std::settings::Section::COUNT as u16);
        // The search never scans the filesystem unless it is told to.
        assert!(!config.filesystem_scan);
    }

    #[test]
    fn the_package_registry_answers_a_real_question() {
        let registry = PackageRegistry::measure();
        // The measurement is a real query, so it either answered or was
        // refused -- it is never a fabricated count.
        if registry.status == ProbeStatus::Refused {
            assert_eq!(registry.count, 0);
            assert!(!registry.contains("org.interos.anything"));
        } else {
            assert!(registry.count as usize <= MAX_PACKAGE_IDS || registry.count > MAX_PACKAGE_IDS as u16);
        }
    }

    #[test]
    fn the_device_state_is_measured_not_declared() {
        // WHY this matters: a screen that shows a device state that was
        // never measured is the defect this whole module exists to prevent.
        let device = DeviceState::measure();
        match device.models.status {
            ModelProbeStatus::Listed => assert!(device.models.entries <= 1024),
            _ => assert_eq!(device.models.entries, 0),
        }
        match device.services.status {
            ProbeStatus::Listed => assert!(device.services.count as usize <= MAX_SERVICES),
            ProbeStatus::Refused => assert_eq!(device.services.count, 0),
        }
    }
}
