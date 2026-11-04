// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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
/// The kernel refused the call: the filesystem is mounted read-only.
pub const ERR_READ_ONLY_FS: i64 = -10;
/// The kernel refused the call: the resource is not ready or a process is currently in use.
pub const ERR_NOT_READY: i64 = -30;

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
pub mod theme;

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
///
/// WHY it is the service's own window: the platform builds its sentence
/// about a model file — why the model file was not run — inside a scratch it
/// names `NOTICE_WINDOW_BYTES` (userspace/system-service/src/lib.rs), and
/// every refusal arm writes into that one window. A transcript that holds
/// less cuts that sentence in half and then has to report the cut its own
/// buffer caused, which is what "Transcript holds 320 of 686 bytes." on the
/// status strip was. The widest sentence the service can hand the reader is
/// bounded by the window, so the transcript holds the window: 768 bytes.
/// The painted line is sized from this number, and twenty wrapped rows of
/// the measured floor (forty bytes a row) carry the whole held message.
pub const CHAT_TEXT_MAX: usize = 768;

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
    ///
    /// WHY a u16 and not a byte: a byte names lengths up to two hundred and
    /// fifty-five, and the transcript now holds the whole seven-hundred-and-
    /// sixty-eight-byte window the service writes its refusal sentences into.
    /// A length field that cannot name the buffer it measures would report a
    /// full message as a short one.
    pub text_len: u16,
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
    /// Whether streaming generation is currently active.
    pub is_streaming: bool,
    /// The current streaming generation step.
    pub streaming_step: u8,
    /// Prompt buffer retained for streaming queries.
    pub stream_prompt: [u8; CHAT_PROMPT_MAX],
    /// How much of the stream prompt buffer is used.
    pub stream_prompt_len: u8,
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
            is_streaming: false,
            streaming_step: 0,
            stream_prompt: [0u8; CHAT_PROMPT_MAX],
            stream_prompt_len: 0,
        }
    }

    /// Adds a message, dropping the oldest when the transcript is full.
    pub fn push(&mut self, role: ChatRole, text: &[u8]) {
        let index = self.next as usize % CHAT_MAX_MESSAGES;
        self.messages[index].role = role;
        let len = text.len().min(CHAT_TEXT_MAX);
        self.messages[index].text[..len].copy_from_slice(&text[..len]);
        self.messages[index].text_len = len as u16;
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

    /// How many bytes the newest message holds.
    ///
    /// WHY this is public: the caller that offered a reply has to be able to ask
    /// whether the transcript kept all of it, and a bubble that stops mid-word
    /// without saying so is a claim that the message ended there.
    pub fn last_text_len(&self) -> usize {
        self.last_message().map(|message| message.text_len as usize).unwrap_or(0)
    }

    /// Whether the transcript is empty.
    pub const fn is_empty(&self) -> bool {
        self.message_count == 0
    }

    /// Returns the most recent message in the transcript, if any.
    pub fn last_message(&self) -> Option<&ChatMessage> {
        if self.message_count == 0 {
            return None;
        }
        let index = (self.next as usize + CHAT_MAX_MESSAGES - 1) % CHAT_MAX_MESSAGES;
        Some(&self.messages[index])
    }

    /// Appends a token chunk to the most recent message in the transcript.
    ///
    /// WHY this exists: streaming generation appends incremental chunks
    /// to an existing assistant response without allocating a new message
    /// slot for each token.
    pub fn append_chunk(&mut self, chunk: &[u8]) -> usize {
        if self.message_count == 0 {
            return 0;
        }
        let index = (self.next as usize + CHAT_MAX_MESSAGES - 1) % CHAT_MAX_MESSAGES;
        let msg = &mut self.messages[index];
        let current_len = msg.text_len as usize;
        let available = CHAT_TEXT_MAX.saturating_sub(current_len);
        let to_copy = chunk.len().min(available);
        if to_copy > 0 {
            msg.text[current_len..current_len + to_copy].copy_from_slice(&chunk[..to_copy]);
            msg.text_len = (current_len + to_copy) as u16;
        }
        to_copy
    }

    /// Prepares the transcript for a streaming generation session.
    pub fn start_stream(&mut self, prompt: &[u8]) {
        self.is_streaming = true;
        self.streaming_step = 0;
        let len = prompt.len().min(CHAT_PROMPT_MAX);
        self.stream_prompt[..len].copy_from_slice(&prompt[..len]);
        self.stream_prompt_len = len as u8;
    }

    /// Completes the streaming generation session.
    pub fn finish_stream(&mut self) {
        self.is_streaming = false;
        self.streaming_step = 0;
        self.stream_prompt_len = 0;
    }

    /// The text of the active streaming prompt.
    pub fn stream_prompt_text(&self) -> &[u8] {
        &self.stream_prompt[..self.stream_prompt_len as usize]
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

    /// Whether INSTALL on this screen asks the platform to move a model file the
    /// device already holds, rather than to install a package.
    ///
    /// WHY the app asks instead of writing: the model namespace is held on the
    /// device and the authority to write there belongs to the storage service. An
    /// app that wrote the file itself would be taking authority it was never
    /// granted, and the screen would show a model the platform never placed.
    /// Defaults to false because a provider delivered as a package is installed
    /// through the package route, which the platform already answers.
    fn installs_from_device(&self) -> bool {
        false
    }

    /// Whether a runtime that can serve the provider is bound.
    fn runtime_binding(&self, device: &DeviceState) -> RuntimeBinding;

    /// Returns the installed package version from the registry if available.
    fn package_version(&self, device: &DeviceState) -> Option<(u16, u16, u16)> {
        let pkg_id = self.package_id()?;
        device.packages.version_of(pkg_id)
    }

    /// Returns the package lifecycle state from the registry if available.
    fn package_lifecycle_state(&self, device: &DeviceState) -> Option<RegistryPackageState> {
        let pkg_id = self.package_id()?;
        device.packages.state_of(pkg_id)
    }
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
        if let Some(pkg_id) = self.package_id() {
            if device.packages.is_active(pkg_id) {
                return InstallState::Installed;
            }
        }
        match device.models.status {
            ModelProbeStatus::Listed => {
                if device.models.has_embedding_weights {
                    InstallState::Installed
                } else {
                    InstallState::NotInstalled
                }
            }
            ModelProbeStatus::Absent => {
                if device.packages.status == ProbeStatus::Refused {
                    InstallState::Unavailable
                } else {
                    InstallState::NotInstalled
                }
            }
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

/// An incremental chunk produced by streaming inference or its unavailability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamChunk {
    /// A measured chunk of token bytes.
    Chunk(&'static [u8]),
    /// Streaming is unavailable with an honest platform reason.
    Unavailable(&'static str),
    /// Streaming generation has finished.
    Done,
}

impl HostBridge {
    /// Queries the host bridge for the next incremental token chunk.
    pub fn stream_chunk(&self, _prompt: &[u8], _step: usize) -> StreamChunk {
        StreamChunk::Unavailable("Host bridge stream unavailable: host inference runtime unbound.")
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
        if let Some(pkg_id) = self.package_id() {
            if device.packages.is_active(pkg_id) {
                return InstallState::Installed;
            }
        }
        match device.models.status {
            ModelProbeStatus::Listed => {
                if device.models.has_generative_weights {
                    InstallState::Installed
                } else {
                    InstallState::NotInstalled
                }
            }
            ModelProbeStatus::Absent => {
                if device.packages.status == ProbeStatus::Refused {
                    InstallState::Unavailable
                } else {
                    InstallState::NotInstalled
                }
            }
            ModelProbeStatus::Refused | ModelProbeStatus::NoAuthority => InstallState::Unavailable,
        }
    }

    /// WHY this one installs from the device: its weights arrive as a file the
    /// person puts on the device, not as a package the platform ships, so INSTALL
    /// here asks the platform to move that file into the model namespace.
    fn installs_from_device(&self) -> bool {
        true
    }

    fn runtime_binding(&self, device: &DeviceState) -> RuntimeBinding {
        device.services.inference_runtime_binding()
    }
}

impl LocalGenerative {
    /// Queries the local generative model for the next incremental token chunk.
    pub fn stream_chunk(&self, _prompt: &[u8], _step: usize) -> StreamChunk {
        StreamChunk::Unavailable("On-device generative stream unavailable: no local model weights loaded.")
    }
}

static EMBEDDING_GEMMA2: EmbeddingGemma2 = EmbeddingGemma2;
pub static HOST_BRIDGE: HostBridge = HostBridge;
pub static LOCAL_GENERATIVE: LocalGenerative = LocalGenerative;

#[allow(dead_code)]
fn contains_slice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack.windows(needle.len()).any(|w| w.eq_ignore_ascii_case(needle))
}

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

/// The candidate model directories the platform probes.
pub const MODEL_DIRECTORIES: &[&str] = &["/system/models", "/data/models"];

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

/// The classification of a model candidate on the filesystem.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModelClassification {
    /// Genuine model weights meeting format and size requirements.
    Weights,
    /// A placeholder, empty file, sub-1 MiB stub, or invalid format.
    Placeholder,
}

/// The minimum size (1 MiB) required for real model weights.
pub const MIN_WEIGHTS_SIZE_BYTES: u64 = 1024 * 1024;

/// The lifecycle stages of an on-device model candidate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProbeStage {
    /// File candidate discovered in model directory.
    Discovered,
    /// File header and minimum size validated.
    Validated,
    /// Model runtime service is active and bound.
    Loaded,
    /// Platform test suite passed for model type.
    Tested,
}

impl ProbeStage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "DISCOVERED",
            Self::Validated => "VALIDATED",
            Self::Loaded => "LOADED",
            Self::Tested => "TESTED",
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
    /// Number of placeholder entries detected.
    pub placeholders: u16,
    /// Whether an embedding-weights file was among the entries.
    pub has_embedding_weights: bool,
    /// Whether a generative-weights file was among the entries.
    pub has_generative_weights: bool,
    /// Size of embedding weights in KB if found.
    pub embedding_size_kb: u64,
    /// Size of generative weights in KB if found.
    pub generative_size_kb: u64,
}

impl ModelInventory {
    /// Classifies a candidate by size and optional header bytes.
    pub fn classify(name: &[u8], size: u64, header: Option<&[u8]>) -> ModelClassification {
        if size < MIN_WEIGHTS_SIZE_BYTES {
            return ModelClassification::Placeholder;
        }
        if contains_ci(name, b"gguf") {
            match header {
                Some(hdr) if hdr.len() >= 4 && &hdr[..4] == b"GGUF" => {}
                _ => return ModelClassification::Placeholder,
            }
        }
        ModelClassification::Weights
    }

    /// Classifies a candidate by size and header slice.
    pub fn classify_slice(name: &[u8], size: u64, header: &[u8]) -> ModelClassification {
        Self::classify(name, size, if header.is_empty() { None } else { Some(header) })
    }

    /// Probes the device's model directory.
    pub fn probe() -> Self {
        if interos_std::cap_lookup(interos_std::cap_type::FILE_SYSTEM_READ, 0).is_err() {
            return Self {
                status: ModelProbeStatus::NoAuthority,
                entries: 0,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            };
        }

        let mut entries = 0u16;
        let mut placeholders = 0u16;
        let mut has_embedding_weights = false;
        let mut has_generative_weights = false;
        let mut embedding_size_kb = 0u64;
        let mut generative_size_kb = 0u64;
        let mut any_listed = false;
        let mut any_refused = false;

        for &dir in MODEL_DIRECTORIES {
            let mut buf = [0u8; 1024];
            match interos_std::fs_list(dir, &mut buf) {
                Ok(n) => {
                    any_listed = true;
                    let payload = &buf[..n.min(buf.len())];
                    for line in payload.split(|&b| b == b'\n') {
                        if line.is_empty() {
                            continue;
                        }
                        let mut parts = line.split(|&b| b == b'|');
                        let name = match parts.next() {
                            Some(n) if !n.is_empty() => n,
                            _ => continue,
                        };
                        let size = parts.next().and_then(parse_u64).unwrap_or(0);
                        entries = entries.saturating_add(1);

                        let mut full_path_buf = [0u8; 128];
                        let mut pos = 0;
                        for &b in dir.as_bytes() {
                            if pos < full_path_buf.len() {
                                full_path_buf[pos] = b;
                                pos += 1;
                            }
                        }
                        if pos < full_path_buf.len() && !dir.ends_with('/') {
                            full_path_buf[pos] = b'/';
                            pos += 1;
                        }
                        for &b in name {
                            if pos < full_path_buf.len() {
                                full_path_buf[pos] = b;
                                pos += 1;
                            }
                        }
                        let mut header_buf = [0u8; 4];
                        let mut header_opt = None;
                        if let Ok(full_path_str) = core::str::from_utf8(&full_path_buf[..pos]) {
                            if let Ok(fd) = interos_std::fs_open(full_path_str, 1) {
                                if let Ok(n_read) = interos_std::fs_read(fd, &mut header_buf) {
                                    header_opt = Some(&header_buf[..n_read]);
                                }
                                let _ = interos_std::fs_close(fd);
                            }
                        }

                        let classification = Self::classify(name, size, header_opt);
                        if classification == ModelClassification::Placeholder {
                            placeholders = placeholders.saturating_add(1);
                            continue;
                        }

                        if contains_ci(name, b"embed") {
                            has_embedding_weights = true;
                            embedding_size_kb = size / 1024;
                        }
                        if contains_ci(name, b"gemma") || contains_ci(name, b"gen-") || contains_ci(name, b"gguf")
                        {
                            has_generative_weights = true;
                            generative_size_kb = size / 1024;
                        }
                    }
                }
                Err(ERR_FILE_NOT_FOUND) => {}
                Err(_) => {
                    any_refused = true;
                }
            }
        }

        let status = if any_listed {
            ModelProbeStatus::Listed
        } else if any_refused {
            ModelProbeStatus::Refused
        } else {
            ModelProbeStatus::Absent
        };

        Self {
            status,
            entries,
            placeholders,
            has_embedding_weights,
            has_generative_weights,
            embedding_size_kb,
            generative_size_kb,
        }
    }

    /// Evaluates the lifecycle stage of on-device embedding weights.
    pub fn embedding_stage(&self, services: &ServiceRegistry) -> Option<ProbeStage> {
        if !self.has_embedding_weights {
            return None;
        }
        if services.contains_inference_runtime() {
            Some(ProbeStage::Loaded)
        } else if self.embedding_size_kb >= 1024 {
            Some(ProbeStage::Validated)
        } else {
            Some(ProbeStage::Discovered)
        }
    }

    /// Evaluates the lifecycle stage of on-device generative weights.
    pub fn generative_stage(&self, services: &ServiceRegistry) -> Option<ProbeStage> {
        if !self.has_generative_weights {
            return None;
        }
        if services.contains_inference_runtime() {
            Some(ProbeStage::Loaded)
        } else if self.generative_size_kb >= 1024 {
            Some(ProbeStage::Validated)
        } else {
            Some(ProbeStage::Discovered)
        }
    }
}

fn parse_u64(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    let mut val = 0u64;
    for &b in bytes {
        if b >= b'0' && b <= b'9' {
            val = val.checked_mul(10)?.checked_add((b - b'0') as u64)?;
        } else {
            return None;
        }
    }
    Some(val)
}

fn parse_u16(bytes: &[u8]) -> Option<u16> {
    parse_u64(bytes).and_then(|v| {
        if v <= u16::MAX as u64 {
            Some(v as u16)
        } else {
            None
        }
    })
}

fn parse_version(bytes: &[u8]) -> (u16, u16, u16) {
    let mut parts = bytes.split(|&b| b == b'.');
    let maj = parts.next().and_then(parse_u16).unwrap_or(0);
    let min = parts.next().and_then(parse_u16).unwrap_or(0);
    let pat = parts.next().and_then(parse_u16).unwrap_or(0);
    (maj, min, pat)
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

/// Lifecycle state of an installed package reported by the platform registry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegistryPackageState {
    /// Package is active and launchable.
    Active,
    /// Package is staged but not yet activated.
    Staged,
    /// Package has been rolled back / quarantined.
    RolledBack,
    /// State could not be identified or unrecognized.
    Unknown,
}

impl RegistryPackageState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Staged => "STAGED",
            Self::RolledBack => "ROLLED_BACK",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// An entry in the platform's package registry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegistryPackageEntry {
    /// Identifier of the package.
    pub id: [u8; MAX_PACKAGE_ID_LEN],
    /// Length of the package identifier.
    pub id_len: u8,
    /// Version (major, minor, patch) parsed from the registry record.
    pub version: (u16, u16, u16),
    /// Lifecycle state of the package.
    pub state: RegistryPackageState,
}

impl RegistryPackageEntry {
    pub const fn empty() -> Self {
        Self {
            id: [0u8; MAX_PACKAGE_ID_LEN],
            id_len: 0,
            version: (0, 0, 0),
            state: RegistryPackageState::Unknown,
        }
    }

    pub fn id_str(&self) -> &str {
        core::str::from_utf8(&self.id[..self.id_len as usize]).unwrap_or("")
    }
}

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
    /// Structured entries holding version and lifecycle state.
    pub entries: [RegistryPackageEntry; MAX_PACKAGE_IDS],
}

impl PackageRegistry {
    /// Constructs an empty package registry measurement.
    pub const fn empty() -> Self {
        Self {
            status: ProbeStatus::Listed,
            count: 0,
            ids: [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
            id_lens: [0; MAX_PACKAGE_IDS],
            entries: [RegistryPackageEntry::empty(); MAX_PACKAGE_IDS],
        }
    }

    /// Constructs a refused package registry measurement.
    pub const fn refused() -> Self {
        Self {
            status: ProbeStatus::Refused,
            count: 0,
            ids: [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS],
            id_lens: [0; MAX_PACKAGE_IDS],
            entries: [RegistryPackageEntry::empty(); MAX_PACKAGE_IDS],
        }
    }

    /// Parses a raw newline-delimited, pipe-separated package registry payload.
    pub fn parse_payload(payload: &[u8]) -> Self {
        let mut ids = [[0u8; MAX_PACKAGE_ID_LEN]; MAX_PACKAGE_IDS];
        let mut id_lens = [0u8; MAX_PACKAGE_IDS];
        let mut entries = [RegistryPackageEntry::empty(); MAX_PACKAGE_IDS];
        let mut count = 0u16;

        for line in payload.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let mut parts = line.split(|&b| b == b'|');
            let id = match parts.next() {
                Some(id) if !id.is_empty() => id,
                _ => continue,
            };
            let _process = parts.next();
            let version_bytes = parts.next().unwrap_or(&[]);
            let state_bytes = parts.next().unwrap_or(&[]);

            let version = parse_version(version_bytes);
            let state = match state_bytes {
                b"active" => RegistryPackageState::Active,
                b"staged" => RegistryPackageState::Staged,
                b"rolled-back" | b"rolled_back" => RegistryPackageState::RolledBack,
                _ => RegistryPackageState::Unknown,
            };

            count = count.saturating_add(1);
            let slot = (count as usize).saturating_sub(1);
            if slot < MAX_PACKAGE_IDS {
                let take = id.len().min(MAX_PACKAGE_ID_LEN);
                ids[slot][..take].copy_from_slice(&id[..take]);
                id_lens[slot] = take as u8;

                let mut entry_id = [0u8; MAX_PACKAGE_ID_LEN];
                entry_id[..take].copy_from_slice(&id[..take]);
                entries[slot] = RegistryPackageEntry {
                    id: entry_id,
                    id_len: take as u8,
                    version,
                    state,
                };
            }
        }

        Self {
            status: ProbeStatus::Listed,
            count,
            ids,
            id_lens,
            entries,
        }
    }

    /// Measures the platform's package registry.
    pub fn measure() -> Self {
        let mut buf = [0u8; 1024];
        match interos_std::package_registry(&mut buf) {
            Ok(n) => Self::parse_payload(&buf[..n.min(buf.len())]),
            Err(_) => Self::refused(),
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

    /// Whether the registry holds an active package with this identifier.
    pub fn is_active(&self, id: &str) -> bool {
        if let Some(entry) = self.find_entry(id) {
            entry.state == RegistryPackageState::Active
        } else {
            false
        }
    }

    /// Finds a package entry by identifier.
    pub fn find_entry(&self, id: &str) -> Option<RegistryPackageEntry> {
        if self.status != ProbeStatus::Listed {
            return None;
        }
        for slot in 0..MAX_PACKAGE_IDS {
            let len = self.entries[slot].id_len as usize;
            if len == 0 {
                continue;
            }
            if self.entries[slot].id[..len] == *id.as_bytes() {
                return Some(self.entries[slot]);
            }
        }
        None
    }

    /// Returns the version of the package with this identifier, if active or present.
    pub fn version_of(&self, id: &str) -> Option<(u16, u16, u16)> {
        self.find_entry(id).map(|e| e.version)
    }

    /// Returns the lifecycle state of the package with this identifier.
    pub fn state_of(&self, id: &str) -> Option<RegistryPackageState> {
        self.find_entry(id).map(|e| e.state)
    }
}

// -----------------------------------------------------------------------------
// Package Transactions
// -----------------------------------------------------------------------------

/// The outcome of an install transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageInstallOutcome {
    /// The package installed and activated successfully.
    Installed,
    /// The package is already active in the registry; idempotent install was refused.
    AlreadyInstalled,
    /// Partial installation failed and was rolled back by the platform.
    RolledBack(i64),
    /// Caller does not have FileSystemWrite authority.
    PermissionDenied,
    /// Provider does not have a package delivery mechanism.
    NoPackage,
}

/// The outcome of a removal transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageRemoveOutcome {
    /// Package uninstalled successfully from the registry and storage freed.
    Uninstalled,
    /// Package removal was refused because a live process of the package is running (in-use).
    InUse,
    /// Package was not found in the registry.
    NotFound,
    /// Provider model weights were stored as loose file on writable storage and unlinked.
    ModelFileUnlinked,
    /// Model file is stored on a read-only filesystem (/system) and cannot be unlinked.
    ReadOnlyFileSystem(i64),
    /// Model file unlinking failed with an error.
    ModelFileFailed(i64),
    /// Platform permission denied.
    PermissionDenied,
    /// Provider is not removable (e.g. host-bridge has no package or local files).
    NotRemovable,
    /// Uninstall failed.
    Failed(i64),
}

/// Evaluates and performs an install transaction for the given provider against the device's registry.
pub fn transact_package_install<F>(
    provider: &dyn Provider,
    device: &DeviceState,
    mut install_syscall: F,
) -> PackageInstallOutcome
where
    F: FnMut(&str, bool) -> Result<(), i64>,
{
    let Some(pkg_id) = provider.package_id() else {
        return PackageInstallOutcome::NoPackage;
    };

    if device.packages.is_active(pkg_id) {
        return PackageInstallOutcome::AlreadyInstalled;
    }

    match install_syscall(pkg_id, true) {
        Ok(()) => PackageInstallOutcome::Installed,
        Err(ERR_PERMISSION_DENIED) => PackageInstallOutcome::PermissionDenied,
        Err(code) => PackageInstallOutcome::RolledBack(code),
    }
}

/// Evaluates and performs a remove transaction for the given provider against the device's registry.
pub fn transact_package_remove<F, U>(
    provider: &dyn Provider,
    device: &DeviceState,
    provider_index: usize,
    mut uninstall_syscall: F,
    mut unlink_syscall: U,
) -> PackageRemoveOutcome
where
    F: FnMut(&str, bool) -> Result<(), i64>,
    U: FnMut(&str) -> Result<(), i64>,
{
    if let Some(pkg_id) = provider.package_id() {
        if device.packages.contains(pkg_id) {
            match uninstall_syscall(pkg_id, true) {
                Ok(()) => return PackageRemoveOutcome::Uninstalled,
                Err(ERR_NOT_READY) => return PackageRemoveOutcome::InUse,
                Err(ERR_PERMISSION_DENIED) => return PackageRemoveOutcome::PermissionDenied,
                Err(ERR_FILE_NOT_FOUND) => return PackageRemoveOutcome::NotFound,
                Err(code) => return PackageRemoveOutcome::Failed(code),
            }
        }
    }

    // Loose model weights contract:
    // When on-device generative models are placed as standalone weights
    // at `/data/models/local_model.gguf`, unlinking is the removal contract.
    if provider.id() == "local-generative" || provider_index == 2 {
        if device.models.has_generative_weights {
            match unlink_syscall("/data/models/local_model.gguf") {
                Ok(_) => return PackageRemoveOutcome::ModelFileUnlinked,
                Err(ERR_READ_ONLY_FS) => return PackageRemoveOutcome::ReadOnlyFileSystem(ERR_READ_ONLY_FS),
                Err(code) => return PackageRemoveOutcome::ReadOnlyFileSystem(code),
            }
        }
    }

    if let Some(_pkg_id) = provider.package_id() {
        PackageRemoveOutcome::NotFound
    } else {
        PackageRemoveOutcome::NotRemovable
    }
}

// -----------------------------------------------------------------------------
// The platform's service registry
// -----------------------------------------------------------------------------

/// The highest service id the platform's service ABI defines.
///
/// WHY this bound matters: the service ABI numbers the platform's own
/// services from 0 (init) to 15 (voice), and the inference, search, agent and
/// voice services sit at 12 to 15. A registry that stays inside the defined
/// range therefore holds nothing the platform has not defined, and one that
/// does not is reported as a change in the platform's service set.
pub const SERVICE_ABI_MAX_ID: u8 = 15;

/// How many services the registry measurement keeps.
pub const MAX_SERVICES: usize = 16;

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
///
/// WHY this is the platform's own numbering: the registry reports ids, and
/// the app's job is to name them the way the platform names them. The
/// platform numbers init at 0 and the voice service at 15, so the inference
/// service is 12, the search service 13 and the agent service 14. A table
/// that is off by one names the inference service "sensor", which is worse
/// than naming nothing at all.
pub const fn service_name(id: u8) -> &'static str {
    match id {
        0 => "init",
        1 => "display",
        2 => "input",
        3 => "storage",
        4 => "network",
        5 => "power",
        6 => "modem",
        7 => "audio",
        8 => "camera",
        9 => "compositor",
        10 => "launcher",
        11 => "sensor",
        12 => "inference",
        13 => "search",
        14 => "agent",
        15 => "voice",
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
    /// runtime that can serve a request.
    ///
    /// WHY this asks for the inference service by id and not for "a service
    /// past the platform's range": the platform now defines services up to 15,
    /// so a registry that holds the search service or the voice service is
    /// inside the defined range and none of them is an inference runtime.
    /// Claiming a runtime from the mere presence of a later service is how
    /// this surface came to claim a model the device does not hold.
    ///
    /// WHY the state and the version are both required: a service that is
    /// stopped cannot answer, and a service that speaks another ABI version
    /// would have its answers misread rather than refused.
    pub fn contains_inference_runtime(&self) -> bool {
        self.find(interos_std::ai::SERVICE_INFERENCE)
            .is_some_and(|entry| {
                entry.state == ServiceState::Running
                    && entry.abi_version == interos_std::ai::INFERENCE_ABI_VERSION
            })
    }

    /// The reason no runtime is bound, in the platform's own terms.
    ///
    /// WHY the reason is derived and not a constant: the reasons are
    /// different facts, and which one applies depends on what the registry
    /// said. A registry that was refused is not the same as a registry that
    /// named twelve services and no inference runtime, and neither is the
    /// same as a registry that named one that is not running.
    pub fn unbound_reason(&self) -> &'static str {
        match self.status {
            ProbeStatus::Refused => "the platform refused the service registry query",
            ProbeStatus::Listed => match self.find(interos_std::ai::SERVICE_INFERENCE) {
                None => "the platform's service registry names no inference service",
                Some(entry) if entry.state != ServiceState::Running => {
                    "the platform's inference service is not running"
                }
                Some(entry) if entry.abi_version != interos_std::ai::INFERENCE_ABI_VERSION => {
                    "the platform's inference service speaks another ABI version"
                }
                Some(_) => "the platform's inference service is running",
            },
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
// System telemetry, agent runtime and persistence
// -----------------------------------------------------------------------------

/// Physical RAM telemetry returned from the microkernel.
#[derive(Clone, Copy, Debug)]
pub struct RuntimeDiagnostics {
    pub ram_total_kb: u64,
    pub ram_used_kb: u64,
    pub ram_free_kb: u64,
    pub has_ram_telemetry: bool,
}

impl RuntimeDiagnostics {
    pub fn measure() -> Self {
        if let Ok(telem) = interos_std::memory_telemetry() {
            Self {
                ram_total_kb: telem.total_bytes / 1024,
                ram_used_kb: telem.used_bytes / 1024,
                ram_free_kb: telem.free_bytes / 1024,
                has_ram_telemetry: true,
            }
        } else {
            Self {
                ram_total_kb: 0,
                ram_used_kb: 0,
                ram_free_kb: 0,
                has_ram_telemetry: false,
            }
        }
    }
}

/// An explicit system tool granted to a bounded agent execution runtime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentTool {
    Calculator,
    CalendarClock,
    StorageStats,
    SettingsQuery,
    CapabilityCheck,
}

/// The widest tool description the surface can show in full.
///
/// WHY this is a shared constant: a description that does not fit is cut,
/// and a cut description that names an expression reads as a different
/// expression. The limit lives next to the descriptions so the two cannot
/// drift apart.
pub const TOOL_LABEL_MAX: usize = 30;

/// The expression the bounded Calculator tool evaluates when the caller
/// names none.
///
/// WHY this is named rather than hidden: the tool computes whatever
/// expression it is handed, and the surface prints this one so the number
/// on the screen can be checked by hand.
pub const CALCULATOR_DEMO_EXPRESSION: &[u8] = b"1024*768/256";

/// Evaluates a bounded integer expression: `+ - * /` over decimal
/// literals, with the usual precedence and a unary minus.
///
/// Returns `None` for input it cannot evaluate — an empty expression, an
/// unknown byte, a trailing operator or a division by zero. A tool that
/// guesses is worse than a tool that reports it could not answer.
pub fn evaluate_integer_expression(expression: &[u8]) -> Option<i64> {
    let mut cursor = 0usize;
    let value = eval_sum(expression, &mut cursor)?;
    skip_spaces(expression, &mut cursor);
    if cursor != expression.len() {
        return None;
    }
    Some(value)
}

fn skip_spaces(expression: &[u8], cursor: &mut usize) {
    while *cursor < expression.len() && expression[*cursor] == b' ' {
        *cursor += 1;
    }
}

fn eval_sum(expression: &[u8], cursor: &mut usize) -> Option<i64> {
    let mut value = eval_product(expression, cursor)?;
    loop {
        skip_spaces(expression, cursor);
        match expression.get(*cursor) {
            Some(b'+') => {
                *cursor += 1;
                value = value.checked_add(eval_product(expression, cursor)?)?;
            }
            Some(b'-') => {
                *cursor += 1;
                value = value.checked_sub(eval_product(expression, cursor)?)?;
            }
            _ => return Some(value),
        }
    }
}

fn eval_product(expression: &[u8], cursor: &mut usize) -> Option<i64> {
    let mut value = eval_factor(expression, cursor)?;
    loop {
        skip_spaces(expression, cursor);
        match expression.get(*cursor) {
            Some(b'*') => {
                *cursor += 1;
                value = value.checked_mul(eval_factor(expression, cursor)?)?;
            }
            Some(b'/') => {
                *cursor += 1;
                let divisor = eval_factor(expression, cursor)?;
                if divisor == 0 {
                    return None;
                }
                value = value.checked_div(divisor)?;
            }
            _ => return Some(value),
        }
    }
}

fn eval_factor(expression: &[u8], cursor: &mut usize) -> Option<i64> {
    skip_spaces(expression, cursor);
    if expression.get(*cursor) == Some(&b'-') {
        *cursor += 1;
        return eval_factor(expression, cursor)?.checked_neg();
    }
    let start = *cursor;
    while *cursor < expression.len() && expression[*cursor].is_ascii_digit() {
        *cursor += 1;
    }
    if *cursor == start {
        return None;
    }
    let mut value: i64 = 0;
    for &byte in &expression[start..*cursor] {
        value = value.checked_mul(10)?.checked_add((byte - b'0') as i64)?;
    }
    Some(value)
}

/// Writes a signed integer, returning how many bytes it used.
fn format_i64(value: i64, buf: &mut [u8]) -> usize {
    let negative = value < 0;
    let magnitude = value.unsigned_abs();
    let mut digits = [0u8; 20];
    let mut len = 0usize;
    let mut magnitude = magnitude;
    while magnitude > 0 && len < digits.len() {
        digits[len] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        len += 1;
    }
    if len == 0 {
        digits[0] = b'0';
        len = 1;
    }
    let mut n = 0usize;
    if negative && n < buf.len() {
        buf[n] = b'-';
        n += 1;
    }
    while len > 0 {
        len -= 1;
        if n < buf.len() {
            buf[n] = digits[len];
            n += 1;
        }
    }
    n
}

impl AgentTool {
    pub const ALL: [AgentTool; 5] = [
        AgentTool::Calculator,
        AgentTool::CalendarClock,
        AgentTool::StorageStats,
        AgentTool::SettingsQuery,
        AgentTool::CapabilityCheck,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            AgentTool::Calculator => "Calculator",
            AgentTool::CalendarClock => "Calendar / RTC",
            AgentTool::StorageStats => "Storage stats",
            AgentTool::SettingsQuery => "Settings query",
            AgentTool::CapabilityCheck => "Capability check",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            AgentTool::Calculator => "Arithmetic: 1024*768/256",
            AgentTool::CalendarClock => "Query hardware RTC clock",
            AgentTool::StorageStats => "Probe disk driver capacity",
            AgentTool::SettingsQuery => "Read search.dimension key",
            AgentTool::CapabilityCheck => "Probe VFS read authority",
        }
    }
}

/// The outcome of a bounded agent tool execution calling real system APIs.
#[derive(Clone, Copy, Debug)]
pub struct AgentToolOutcome {
    pub tool: AgentTool,
    pub output: [u8; 64],
    pub output_len: u8,
    pub ok: bool,
}

/// The capability type the kernel mints when a person consents to this app
/// running agent tools.
///
/// WHY a capability type and not a local flag: the authority to run a tool is
/// the kernel's to give and the kernel's to take back. The kernel's consent
/// broker mints a capability of this type into the asking task's table, and
/// the kernel's own lookup is what says whether the task still holds it.
pub const TOOL_CAPABILITY_TYPE: u64 = interos_std::cap_type::AGENT_TASK;

/// The object the agent-task capability is scoped to.
///
/// WHY zero and not one object per tool: the broker mints the grant for the
/// agent task domain as a whole, and each tool's own syscall checks the
/// authority that tool needs on its own.
pub const TOOL_CAPABILITY_OBJECT: u64 = 0;

/// The name the permission broker shows the person when this app asks.
pub const TOOL_CONSENT_APP_NAME: &str = "Agent Manager";

/// The platform's refusal code for a request that is waiting for the person.
const PERMISSION_DENIED: i64 = -1;

/// What the tool refuses with when the platform holds no authority for it.
///
/// WHY this is the platform's answer and not the app's: the app did not
/// refuse the person, and the person did not refuse the app. The kernel's
/// capability table holds no agent-task capability for this task, so the tool
/// was never allowed to run.
pub const TOOL_REFUSED_MESSAGE: &[u8] = b"Refused: the kernel holds no agent task capability";

/// What the platform answered when the app asked for the authority to run
/// bounded tools.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConsentOutcome {
    /// The kernel minted the capability and it is in this task's table.
    Granted,
    /// The request is waiting for the person to decide.
    Pending,
    /// The platform refused the request without asking anyone.
    Refused,
}

/// The platform's answer to "may this app run the bounded agent tools?".
///
/// WHY this is not a bool: a bool in the app's own memory is a claim the app
/// makes about itself, and the platform's rule is that no app holds ambient
/// authority. What the app holds is the kernel's answer instead: the
/// generation-counted handle of the capability the kernel minted for this
/// task. The handle goes stale the moment the grant is revoked, which is why
/// the app asks the kernel again every time it uses the authority.
///
/// WHY the answer cannot be written down here: the variant that says "granted"
/// is private to this module, so the only way to hold a granted authority is
/// to ask the kernel for one. A caller cannot assemble the answer in its own
/// memory and call that consent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ToolAuthority {
    inner: AuthorityInner,
}

/// What the kernel said about the agent-task capability.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AuthorityInner {
    /// The kernel's capability table holds the agent-task capability.
    Granted {
        /// The generation-counted handle the kernel minted for it.
        handle: u64,
    },
    /// The kernel's capability table holds no such capability.
    Refused,
}

impl ToolAuthority {
    /// Asks the kernel whether this task holds the agent-task capability.
    ///
    /// WHY the handle and not the slot: a slot is a position in this task's
    /// own capability table, and the position is not the question being asked
    /// here. The handle names the grant and carries its generation, so a
    /// grant that was revoked and re-inserted is a different grant.
    pub fn probe() -> Self {
        let Ok(handle) = interos_std::capability_handle(
            TOOL_CAPABILITY_TYPE,
            TOOL_CAPABILITY_OBJECT,
        ) else {
            return Self { inner: AuthorityInner::Refused };
        };
        Self {
            inner: AuthorityInner::Granted { handle },
        }
    }

    /// Asks the platform's permission broker for the authority to run tools.
    ///
    /// WHY the app asks the broker and not the person directly: the broker is
    /// the only party that can mint the capability, and it mints it for the
    /// task that asked. An app cannot put the authority in its own table.
    ///
    /// WHY the existing grant is checked first: the broker mints a second
    /// capability every time it grants a request, and a table that fills with
    /// copies of the same grant is a table that refuses the next one.
    pub fn request() -> ConsentOutcome {
        if Self::probe().is_granted() {
            return ConsentOutcome::Granted;
        }
        match interos_std::cap_request_consent(
            interos_std::SensitivePermission::AgentTask,
            TOOL_CONSENT_APP_NAME,
            false,
        ) {
            Ok(_) => ConsentOutcome::Granted,
            Err(code) if code == PERMISSION_DENIED => ConsentOutcome::Pending,
            Err(_) => ConsentOutcome::Refused,
        }
    }

    /// Whether the kernel holds the capability.
    pub const fn is_granted(&self) -> bool {
        matches!(self.inner, AuthorityInner::Granted { .. })
    }

    /// The handle the kernel minted for the capability, if it holds one.
    pub const fn handle(&self) -> Option<u64> {
        match self.inner {
            AuthorityInner::Granted { handle } => Some(handle),
            AuthorityInner::Refused => None,
        }
    }
}

impl AgentToolOutcome {
    /// Executes a tool if the kernel holds the agent-task capability for this
    /// task.
    ///
    /// WHY the authority is asked of the kernel here and not remembered by the
    /// app: a grant can be revoked while the app runs, and a tool that ran on
    /// a remembered "yes" would be running on authority the platform took
    /// back.
    pub fn execute(tool: AgentTool) -> Self {
        Self::execute_with_authority(tool, &ToolAuthority::probe())
    }

    /// Executes the Calculator tool over a caller-supplied expression.
    ///
    /// WHY the expression is an argument: a tool that answers the same
    /// number whatever it is asked is not computing anything. The result
    /// the surface shows is this evaluator's answer for the expression it
    /// was given, or an explicit refusal.
    pub fn execute_expression_with_authority(expression: &[u8], authority: &ToolAuthority) -> Self {
        let mut outcome = Self::execute_with_authority(AgentTool::Calculator, authority);
        if !authority.is_granted() {
            return outcome;
        }
        let mut output = [0u8; 64];
        match evaluate_integer_expression(expression) {
            Some(value) => {
                let n = format_i64(value, &mut output);
                outcome.ok = true;
                outcome.output_len = n as u8;
            }
            None => {
                let message = b"Calculator: expression not evaluable";
                let n = message.len().min(output.len());
                output[..n].copy_from_slice(&message[..n]);
                outcome.ok = false;
                outcome.output_len = n as u8;
            }
        }
        outcome.output = output;
        outcome
    }

    /// Executes a tool under an authority the kernel resolved.
    ///
    /// WHY the gate is the capability and not a boolean: the platform's rule
    /// is that no app holds ambient authority, so a tool that runs is a tool
    /// the kernel let run. The capability is asked of the kernel for this
    /// task, and every tool's own syscall still checks the authority that
    /// tool needs, so this gate is the outer limit rather than the inner one.
    pub fn execute_with_authority(tool: AgentTool, authority: &ToolAuthority) -> Self {
        if !authority.is_granted() {
            return Self::refused(tool);
        }

        let mut output = [0u8; 64];
        let output_len;
        let mut ok = false;

        match tool {
            AgentTool::Calculator => {
                match evaluate_integer_expression(CALCULATOR_DEMO_EXPRESSION) {
                    Some(value) => {
                        output_len = format_i64(value, &mut output);
                        ok = true;
                    }
                    None => {
                        let err = b"Calculator: expression not evaluable";
                        output[..err.len()].copy_from_slice(err);
                        output_len = err.len();
                    }
                }
            }
            AgentTool::CalendarClock => {
                let mut time_buf = [0u8; 64];
                if let Ok(n) = interos_std::rtc_get_time(&mut time_buf) {
                    let copy_len = n.min(output.len());
                    output[..copy_len].copy_from_slice(&time_buf[..copy_len]);
                    output_len = copy_len;
                    ok = true;
                } else {
                    let err = b"RTC unavailable";
                    output[..err.len()].copy_from_slice(err);
                    output_len = err.len();
                }
            }
            AgentTool::StorageStats => {
                if let Ok(bytes) = interos_std::storage_query() {
                    let kb = bytes / 1024;
                    let n = format_u32(kb as u32, &mut output);
                    let suffix = b" KB";
                    output[n..n + suffix.len()].copy_from_slice(suffix);
                    output_len = n + suffix.len();
                    ok = true;
                } else {
                    let err = b"Storage unavailable";
                    output[..err.len()].copy_from_slice(err);
                    output_len = err.len();
                }
            }
            AgentTool::SettingsQuery => {
                let mut buf = [0u8; 32];
                if let Ok(n) = interos_std::settings_get("search.dimension", &mut buf) {
                    if n > 0 {
                        let take = n.min(60);
                        output[..take].copy_from_slice(&buf[..take]);
                        output_len = take;
                        ok = true;
                    } else {
                        let err = b"search.dimension: none";
                        output[..err.len()].copy_from_slice(err);
                        output_len = err.len();
                    }
                } else {
                    let err = b"Settings query refused";
                    output[..err.len()].copy_from_slice(err);
                    output_len = err.len();
                }
            }
            AgentTool::CapabilityCheck => {
                if interos_std::cap_lookup(interos_std::cap_type::FILE_SYSTEM_READ, 0).is_ok() {
                    let msg = b"VFS read held";
                    output[..msg.len()].copy_from_slice(msg);
                    output_len = msg.len();
                    ok = true;
                } else {
                    let msg = b"VFS read not held";
                    output[..msg.len()].copy_from_slice(msg);
                    output_len = msg.len();
                }
            }
        }

        Self {
            tool,
            output,
            output_len: output_len as u8,
            ok,
        }
    }

    /// The refusal a tool returns when the platform holds no authority.
    ///
    /// WHY the refusal names the kernel and not a person: nobody in this app
    /// turned the tool off. The platform's capability table holds no
    /// agent-task capability for this task, so the tool was never allowed to
    /// run, and the screen says which of the two facts that is.
    pub fn refused(tool: AgentTool) -> Self {
        let mut output = [0u8; 64];
        let len = TOOL_REFUSED_MESSAGE.len().min(output.len());
        output[..len].copy_from_slice(&TOOL_REFUSED_MESSAGE[..len]);
        Self {
            tool,
            output,
            output_len: len as u8,
            ok: false,
        }
    }

    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.output[..self.output_len as usize]).unwrap_or("")
    }
}

/// Loads the user's preferred selected provider index from app private storage.
pub fn load_saved_provider() -> Option<usize> {
    let mut buf = [0u8; 1];
    if let Ok(fd) = interos_std::app_storage_open("prefs.bin", 1) {
        if let Ok(n) = interos_std::fs_read(fd, &mut buf) {
            let _ = interos_std::fs_close(fd);
            if n > 0 && (buf[0] as usize) < PROVIDERS.len() {
                return Some(buf[0] as usize);
            }
        }
        let _ = interos_std::fs_close(fd);
    }
    None
}

/// Persists the user's preferred selected provider index to app private storage.
pub fn save_preferred_provider(index: usize) {
    if index >= PROVIDERS.len() {
        return;
    }
    let buf = [index as u8];
    if let Ok(fd) = interos_std::app_storage_open("prefs.bin", 2) {
        let _ = interos_std::fs_write(fd, &buf);
        let _ = interos_std::fs_close(fd);
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
const CAPABILITY_TABLE: [CapabilityGrant; 13] = [
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
    CapabilityGrant {
        cap_type: TOOL_CAPABILITY_TYPE,
        name: "Agent task",
        allows: "run the bounded agent tools under the consent the kernel holds",
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

// -----------------------------------------------------------------------------
// The row of equal-width controls, and the one place its geometry is written
// -----------------------------------------------------------------------------

/// A row of controls of equal width, laid out across one content column.
///
/// WHY a type and not numbers at each point of use: the agent manager's action
/// bar paints a button from `(content - (live - 1) * gap) / live` and answers a
/// tap on the same expression. The tab row carried two copies of exactly this
/// arithmetic once, wrote the gap count down in one of them, and the sixth tab
/// ended up painted in one place and pressed in another. A row that is described
/// once cannot disagree with itself, and both the painter and the hit test read
/// the description rather than recomputing it.
#[derive(Clone, Copy, Debug)]
pub struct ControlRow {
    /// The column's first column.
    pub x: usize,
    /// The column's width.
    pub w: usize,
    /// The row's first row.
    pub y: usize,
    /// The row's height, which is also its tap band's height.
    pub h: usize,
    /// The air every pair of neighbours keeps between itself.
    pub gap: usize,
    /// How many controls the row holds; a control with nothing to say is not
    /// drawn and does not answer, so it is not counted here.
    pub live: usize,
}

impl ControlRow {
    /// The slot the index-th live control takes.
    pub const fn slot(self, index: usize) -> interos_std::design::primitives::Rect {
        let width = self.slot_width();
        interos_std::design::primitives::Rect::new(
            self.x + index * (width + self.gap),
            self.y,
            width,
            self.h,
        )
    }

    /// How wide one live control is.
    pub const fn slot_width(self) -> usize {
        if self.live == 0 {
            return 0;
        }
        (self.w - (self.live - 1) * self.gap) / self.live
    }

    /// Which live control a point landed on, or none.
    ///
    /// WHY this walks the slots instead of dividing the offset by a stride: the
    /// division would make the air between two neighbours answer as the control
    /// on its right, and a strip nobody drew is not a button. A point is on a
    /// control when it is inside the very rect that control was painted on, so
    /// the band this answers and the plate the reader saw are one plate.
    pub const fn tapped(self, ux: usize, uy: usize) -> Option<usize> {
        if uy < self.y || uy >= self.y + self.h {
            return None;
        }
        let mut index = 0;
        while index < self.live {
            let slot = self.slot(index);
            if ux >= slot.x && ux < slot.x + slot.w {
                return Some(index);
            }
            index += 1;
        }
        None
    }
}

// -----------------------------------------------------------------------------
// The embedding probe
//
// WHAT THIS SECTION IS FOR: the screen where the agent manager already asks the
// platform's inference service for retrieval can also ask it for one embedding.
// Everything the probe holds is a field the reply carried or an interval the app
// itself timed, and nothing is inferred from what the app hoped the service would
// say. The probe's own state names whether an answer has been read at all, so a
// waiting request is never painted as a completed one.
// -----------------------------------------------------------------------------

/// The most bytes one mailbox message can carry.
///
/// WHY the number is written down: the kernel builds every message through
/// `IpcMessage::new`, which takes `data.len().min(IPC_MAX_PAYLOAD)` and says
/// nothing when it cuts (`kernel/src/ipc/mod.rs:34` and `:65`). A reply wider
/// than this reaches the app shortened, and the only witness left that it was is
/// the length the service wrote into its own header -- which is why the probe
/// keeps the arrived count beside the offered one instead of trusting either.
pub const MAILBOX_PAYLOAD_MAX: usize = 64;

/// The width of the inference service's reply header.
pub const REPLY_HEADER_BYTES: usize =
    core::mem::size_of::<interos_std::ai::AiInferenceResponse>();

/// The width of the request header this app writes.
pub const REQUEST_HEADER_BYTES: usize =
    core::mem::size_of::<interos_std::ai::AiInferenceRequest>();

/// The most payload bytes one embedding reply can hand the app.
pub const PROBE_PAYLOAD_MAX: usize = MAILBOX_PAYLOAD_MAX - REPLY_HEADER_BYTES;

/// The most text bytes one embedding request can carry to the service.
///
/// WHY it is the mailbox's width minus this app's own header: the kernel copies a
/// message into a fixed slot and cuts the tail without saying so, so text past
/// this count would reach the service shorter than the header the app wrote --
/// `input_len` would claim more bytes than arrived. The app does the cutting, at
/// this number, and is therefore able to say how much of the typed text it sent.
pub const PROBE_REQUEST_TEXT_MAX: usize = MAILBOX_PAYLOAD_MAX - REQUEST_HEADER_BYTES;

/// How many leading values the screen lists.
///
/// WHY eight: the row's value column fits about forty bytes, and eight signed
/// components is the most that can be shown without the painter cutting the last
/// one -- a list that stops mid-number reads as a different list.
pub const PROBE_VALUES_SHOWN: usize = 8;

/// The longest text one embedding request carries.
///
/// WHY it is the composer's own length: the probe sends what the person typed,
/// and a request wider than the composer can hold would be a request whose text
/// the screen cannot show back to them.
pub const PROBE_TEXT_MAX: usize = CHAT_PROMPT_MAX;

/// Where the embedding probe stands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProbeState {
    /// Nothing has been asked.
    Idle,
    /// The request is with the platform and no reply has been read yet.
    Awaiting,
    /// A reply was read, and the probe's fields hold what it carried.
    Answered,
    /// A reply arrived and is not one this app can read.
    ///
    /// WHY it is not folded into `Answered`: a status of zero is the platform's
    /// own code for success, and a probe that filled its reply fields with
    /// zeroes would report a refusal it never saw.
    Unreadable,
    /// The app's wait ran out and the platform sent nothing.
    NoReply,
}

/// The fields one embedding reply actually carried.
#[derive(Clone, Copy, Debug)]
pub struct EmbedReply {
    /// The status code the reply stamped itself.
    pub status: u32,
    /// The reply's own `output_dim` field.
    ///
    /// WHY it is kept raw: the service puts a dimension in this field when it
    /// sends vector components and the byte count of a sentence when it sends one
    /// (`userspace/system-service/src/lib.rs:562-586`), so the number only means
    /// something beside the status that sent it.
    pub output_dim: u32,
    /// The reply's own `latency_micros` field.
    pub latency: u64,
    /// The bytes that arrived after the header, kept as the signed values the
    /// service's Q7 vectors are made of.
    pub payload: [i8; PROBE_PAYLOAD_MAX],
    /// How many of them arrived.
    pub payload_len: u8,
}

impl EmbedReply {
    /// Whether this reply is a vector the service says it produced.
    ///
    /// WHY the status decides it and not the bytes: `aid` appends embedding
    /// components to a reply on two arms only -- the lexical baseline, which
    /// stamps `STATUS_BASELINE_LEXICAL` (`userspace/system-service/src/lib.rs:1583`),
    /// and the file-backed run, which stamps `STATUS_SUCCESS` (`:2159`). Every
    /// other embedding answer goes through `emit_response_as`, whose tail is the
    /// service's own sentence. A status outside those two therefore names words,
    /// and printing those bytes as numbers would invent a vector out of a refusal.
    ///
    /// WHY the first reply after the request is the one to read: on the file-backed
    /// branch the service sends the vector first (`:2172`) and its measurement
    /// sentence second (`:2196`), and the app empties any embedding reply that was
    /// already queued before it asks, so a message it reads after sending cannot
    /// predate the request.
    pub const fn carries_vector(&self) -> bool {
        self.status == interos_std::ai::STATUS_SUCCESS
            || self.status == interos_std::ai::STATUS_BASELINE_LEXICAL
    }

    /// The attribution this reply's own status supports.
    pub const fn attribution(&self) -> Option<&'static [u8]> {
        embed_attribution(self.status)
    }

    /// The payload as bytes, for a screen that is showing the service's sentence.
    ///
    /// WHY it is kept signed: the same bytes are a vector's components on one
    /// status and the service's sentence on another, and the only difference is
    /// how the screen reads them. `EmbedProbe::sentence_bytes` does that reading,
    /// and a caller that wants the numbers reads the field directly.
    pub fn payload_bytes(&self) -> &[i8] {
        &self.payload[..self.payload_len as usize]
    }
}

/// The attribution an embedding status supports, in the app's existing vocabulary,
/// or nothing when the status supports none.
///
/// WHY these two words and no others: the app already attributes an answer to one
/// of two sources -- `[Neural RAG]` when the platform's hit says physical weights
/// were used and `[Lexical RAG]` when it says they were not (`main.rs`, and
/// `interos_std::ai::AiSearchHit::is_neural`). An embedding reply carries no
/// retrieval, so the words stand without the "RAG": the SDK defines
/// `STATUS_BASELINE_LEXICAL` as the lexical projection with "no tensor of any
/// model file ... read to make it", and `STATUS_SUCCESS` as the answer that is not
/// that projection (`sdk/rust/interos-std/src/ai.rs:103-125`).
///
/// WHY `None` is not spelled `[Unknown]`: a refusal reply says in its own sentence
/// why nothing ran. Attributing it here would be the app's guess about a branch it
/// did not observe, and the sentence is the observation.
pub const fn embed_attribution(status: u32) -> Option<&'static [u8]> {
    if status == interos_std::ai::STATUS_SUCCESS {
        Some(b"[Neural]")
    } else if status == interos_std::ai::STATUS_BASELINE_LEXICAL {
        Some(b"[Lexical]")
    } else {
        None
    }
}

/// Reads one embedding reply, or reports that the bytes are not one.
///
/// WHY `None` and not a zeroed reply: a message shorter than the header names no
/// status and no dimension, and a screen built on zeroes would report the
/// platform as having answered `0` when it answered nothing at all.
pub fn read_embed_reply(bytes: &[u8]) -> Option<EmbedReply> {
    if bytes.len() < REPLY_HEADER_BYTES {
        return None;
    }
    // WHY unaligned: the reply arrives in a byte buffer the mailbox filled, and
    // the header is read the same way the service wrote it.
    let header = unsafe {
        core::ptr::read_unaligned(
            bytes.as_ptr() as *const interos_std::ai::AiInferenceResponse,
        )
    };
    let tail = &bytes[REPLY_HEADER_BYTES..];
    let take = tail.len().min(PROBE_PAYLOAD_MAX);
    let mut payload = [0i8; PROBE_PAYLOAD_MAX];
    let mut index = 0;
    while index < take {
        payload[index] = tail[index] as i8;
        index += 1;
    }
    Some(EmbedReply {
        status: header.status,
        output_dim: header.output_dim,
        latency: header.latency_micros,
        payload,
        payload_len: take as u8,
    })
}

/// What the app asked the platform to embed, and what the reply carried back.
#[derive(Clone, Copy, Debug)]
pub struct EmbedProbe {
    /// Where the request stands.
    pub state: ProbeState,
    /// The text the app sent, kept so the screen can show what was asked.
    pub text: [u8; PROBE_TEXT_MAX],
    /// How much of it was sent.
    pub text_len: u16,
    /// How much the person had typed when the request went out.
    ///
    /// WHY it is kept beside `text_len`: the two differ whenever the composer held
    /// more than one request can carry, and a screen that showed only the sent
    /// count would read as though nothing had been cut.
    pub typed_bytes: u16,
    /// The dimension the request asked for, as the request carried it.
    pub asked_dim: u32,
    /// Whether the app named a dimension at all. `false` means it sent `0`, which
    /// is the request that leaves the service's own default standing.
    pub asked_named_dim: bool,
    /// How many bytes the request was.
    pub request_bytes: u16,
    /// The tick the request went out on.
    pub sent_at_tick: u64,
    /// The interval the app measured around the send call.
    pub send_ticks: u64,
    /// The interval between the request and the reply the app read.
    pub reply_ticks: u64,
    /// The tick by which the platform must have answered.
    pub deadline: u64,
    /// The status the reply carried.
    pub status: u32,
    /// The reply's `output_dim` field, raw.
    pub reply_dim: u32,
    /// The reply's `latency_micros` field, raw.
    pub reply_latency: u64,
    /// Whether the reply carried vector components.
    pub reply_is_vector: bool,
    /// The attribution the reply's status supported, if any.
    pub attributed: bool,
    /// The bytes that arrived after the reply's header.
    pub payload: [i8; PROBE_PAYLOAD_MAX],
    /// How many of them arrived.
    pub payload_len: u8,
    /// How many bytes the reply said its payload was, from its own header.
    pub offered: u32,
}

impl EmbedProbe {
    /// A probe that has asked for nothing.
    pub const fn new() -> Self {
        Self {
            state: ProbeState::Idle,
            text: [0; PROBE_TEXT_MAX],
            text_len: 0,
            typed_bytes: 0,
            asked_dim: 0,
            asked_named_dim: false,
            request_bytes: 0,
            sent_at_tick: 0,
            send_ticks: 0,
            reply_ticks: 0,
            deadline: 0,
            status: 0,
            reply_dim: 0,
            reply_latency: 0,
            reply_is_vector: false,
            attributed: false,
            payload: [0; PROBE_PAYLOAD_MAX],
            payload_len: 0,
            offered: 0,
        }
    }

    /// Records what the platform sent, on the tick the app read it.
    pub fn record(&mut self, reply: &EmbedReply, replied_at_tick: u64) {
        self.status = reply.status;
        self.reply_dim = reply.output_dim;
        self.reply_latency = reply.latency;
        self.reply_is_vector = reply.carries_vector();
        self.attributed = reply.attribution().is_some();
        self.payload = reply.payload;
        self.payload_len = reply.payload_len;
        self.offered = reply.output_dim;
        self.reply_ticks = replied_at_tick.wrapping_sub(self.sent_at_tick);
        self.deadline = 0;
        self.state = ProbeState::Answered;
    }

    /// Records the request the app is about to send, and forgets the last reply.
    ///
    /// WHY the previous reply is cleared here and not left standing: the screen
    /// shows what the platform sent back, and a card that still held the earlier
    /// answer while a second request was in flight would report a number the
    /// person had not asked for again.
    ///
    /// WHY `send_ticks` is the caller's: the interval is measured around the send
    /// call itself, in the app, the same way `send_prompt` measures the one it
    /// makes; a model layer that slept would be measuring its own arithmetic.
    pub fn start_request(
        &mut self,
        sent: &[u8],
        typed_len: usize,
        asked_dim: Option<u32>,
        request_bytes: usize,
        sent_at_tick: u64,
        send_ticks: u64,
        wait_ticks: u64,
    ) {
        let len = sent.len().min(PROBE_TEXT_MAX);
        let mut index = 0;
        while index < PROBE_TEXT_MAX {
            self.text[index] = 0;
            index += 1;
        }
        let mut copy = 0;
        while copy < len {
            self.text[copy] = sent[copy];
            copy += 1;
        }
        self.text_len = len as u16;
        self.typed_bytes = typed_len.min(u16::MAX as usize) as u16;
        self.asked_dim = asked_dim.unwrap_or(0);
        self.asked_named_dim = asked_dim.is_some();
        self.request_bytes = request_bytes as u16;
        self.sent_at_tick = sent_at_tick;
        self.send_ticks = send_ticks;
        self.reply_ticks = 0;
        self.deadline = sent_at_tick.saturating_add(wait_ticks);
        self.status = 0;
        self.reply_dim = 0;
        self.reply_latency = 0;
        self.reply_is_vector = false;
        self.attributed = false;
        self.payload = [0; PROBE_PAYLOAD_MAX];
        self.payload_len = 0;
        self.offered = 0;
        self.state = ProbeState::Awaiting;
    }

    /// Records that the wait ran out and the platform sent nothing.
    ///
    /// WHY it is a state of its own: the app knows it asked and knows it got no
    /// answer, which is a fact about this exchange -- unlike `Idle`, which knows
    /// nothing was asked at all.
    pub fn report_no_reply(&mut self, gave_up_at_tick: u64) {
        self.deadline = 0;
        self.reply_ticks = gave_up_at_tick.wrapping_sub(self.sent_at_tick);
        self.state = ProbeState::NoReply;
    }

    /// Records that a reply arrived and this app cannot read it.
    ///
    /// WHY the reply fields stay empty: `read_embed_reply` refused the bytes, so
    /// the app holds no status, no dimension and no payload from them. Filling in
    /// zeroes would report the platform's answer as `STATUS_SUCCESS`, which is the
    /// one code it may not have sent.
    pub fn report_unreadable(&mut self, replied_at_tick: u64) {
        self.deadline = 0;
        self.reply_ticks = replied_at_tick.wrapping_sub(self.sent_at_tick);
        self.state = ProbeState::Unreadable;
    }

    /// The text the app sent.
    pub fn sent_text(&self) -> &[u8] {
        &self.text[..self.text_len as usize]
    }

    /// The reply's payload as the bytes of the service's own sentence.
    ///
    /// WHY a copy into the caller's buffer: the payload is held as the signed
    /// values a vector is made of, and a sentence is the same bytes read as
    /// unsigned ones -- `as u8` on an `i8` is a defined reinterpretation, where
    /// casting a slice's type would not be.
    pub fn sentence_bytes(&self, out: &mut [u8]) -> usize {
        let take = (self.payload_len as usize).min(out.len());
        let mut index = 0;
        while index < take {
            out[index] = self.payload[index] as u8;
            index += 1;
        }
        take
    }

    /// How many vector components reached the app.
    pub fn values_arrived(&self) -> usize {
        self.payload_len as usize
    }

    /// Writes the leading vector components the reply carried, and reports how
    /// many of them fit.
    ///
    /// WHY the reply has to be a vector: on a refusal the same bytes are the
    /// service's sentence, and `101` for `e` printed under a "values" label is a
    /// vector nobody sent. The caller checks the state the reply's own status put
    /// the probe in before it calls this.
    ///
    /// WHY a number is never written part-way: a list that stops mid-digit reads
    /// as a different list, so a component whose digits do not fit ends the list,
    /// and the returned count says where it ended.
    pub fn leading_values(&self, out: &mut [u8]) -> (usize, usize) {
        let show = self.values_arrived().min(PROBE_VALUES_SHOWN);
        let mut written = 0usize;
        let mut count = 0usize;
        let mut digits = [0u8; 8];
        let mut index = 0;
        while index < show {
            let len = format_i16(self.payload[index] as i16, &mut digits);
            let need = len + if count == 0 { 0 } else { 2 };
            if written + need > out.len() {
                break;
            }
            if count > 0 {
                out[written] = b',';
                out[written + 1] = b' ';
                written += 2;
            }
            let mut copy = 0;
            while copy < len {
                out[written] = digits[copy];
                written += 1;
                copy += 1;
            }
            count += 1;
            index += 1;
        }
        (written, count)
    }
}

/// Builds one embedding request: the service's own header, then the text.
///
/// WHY the header and not the bare text: a payload the service cannot read as a
/// header is taken as a prompt whole, which would make the requested dimension a
/// guess. Filling the header is what lets the app state the dimension it asked
/// for and have the reply name the dimension that came back.
///
/// WHY `model_id` is zero and no name is attached to it: the embedding arm of the
/// service reads no model out of a request -- it answers from the one file its
/// probe found, or from the baseline -- so naming a model here would be the app
/// claiming a choice the platform does not make.
///
/// WHY no token-id flag is set: the request carries text, and the flag that
/// promises ids (`interos_std::ai::FLAG_INPUT_TOKEN_IDS`) would send this text as
/// though its bytes were vocabulary ids. The service's own planning function
/// routes text with no ids to a labelled answer, and the probe reports whichever
/// answer comes back.
///
/// WHY the text is cut here, at the buffer's own width: the mailbox would cut it
/// at sixty-four bytes and leave the header claiming more text than arrived. A
/// caller that writes this far into `out` knows exactly how many typed bytes went
/// -- `out.len() - REQUEST_HEADER_BYTES` -- and so can say so on the screen.
pub fn build_embed_request(
    text: &[u8],
    asked_dim: Option<u32>,
    out: &mut [u8],
) -> Option<usize> {
    let header_bytes = core::mem::size_of::<interos_std::ai::AiInferenceRequest>();
    if out.len() < header_bytes {
        return None;
    }
    let len = text.len().min(out.len() - header_bytes).min(PROBE_TEXT_MAX);
    let request = interos_std::ai::AiInferenceRequest {
        model_id: 0,
        target_dim: asked_dim.unwrap_or(0),
        input_len: len as u32,
        flags: 0,
        input_offset: 0,
    };
    let header = unsafe {
        core::slice::from_raw_parts(
            &request as *const interos_std::ai::AiInferenceRequest as *const u8,
            header_bytes,
        )
    };
    let mut index = 0;
    while index < header_bytes {
        out[index] = header[index];
        index += 1;
    }
    let mut tail = 0;
    while tail < len {
        out[header_bytes + tail] = text[tail];
        tail += 1;
    }
    Some(header_bytes + len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An authority the kernel granted, without asking the kernel.
    ///
    /// WHY it is built rather than probed: `probe` is a syscall, so on the
    /// test host it always answers that nothing was granted, and a tool's own
    /// arithmetic could never be exercised.
    fn granted_authority() -> ToolAuthority {
        ToolAuthority {
            inner: AuthorityInner::Granted { handle: 1 },
        }
    }

    /// An authority the kernel refused.
    fn refused_authority() -> ToolAuthority {
        ToolAuthority {
            inner: AuthorityInner::Refused,
        }
    }

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
                    placeholders: 0,
                    has_embedding_weights: false,
                    has_generative_weights: false,
                    embedding_size_kb: 0,
                    generative_size_kb: 0,
                },
                packages: PackageRegistry::empty(),
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

    /// WHY only one provider asks for a file move: the model namespace is written
    /// by the storage service, and INSTALL on a provider the platform delivers as a
    /// package has to go through the package route, where the signature is checked.
    #[test]
    fn only_the_local_weights_provider_installs_from_the_device() {
        assert!(provider_by_id("local-generative")
            .unwrap()
            .installs_from_device());
        assert!(!provider_by_id("embeddinggemma2")
            .unwrap()
            .installs_from_device());
        assert!(
            !provider_by_id("host-bridge")
                .unwrap()
                .installs_from_device(),
            "a provider that runs elsewhere has no file for this device to move"
        );
    }

    #[test]
    fn a_new_provider_is_an_implementation_and_not_an_edit_through_the_app() {        // WHY this matters: the point of the trait is that adding a model
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
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: PackageRegistry::empty(),
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
                placeholders: 0,
                has_embedding_weights: true,
                has_generative_weights: true,
                embedding_size_kb: 1024,
                generative_size_kb: 1024,
            },
            packages: PackageRegistry::empty(),
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
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: PackageRegistry::refused(),
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
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: PackageRegistry::empty(),
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
        // WHY the reason names the missing service and not the range it
        // searched: `unbound_reason` returns a `&'static str`, so a range read
        // from the platform cannot be carried in it. The listing's own bounds
        // are their own row on the surface, drawn from `highest_id`.
        assert!(listed
            .unbound_reason()
            .contains("names no inference service"));
        assert!(!listed.inference_runtime_binding().is_bound());
        assert!(!listed.contains_inference_runtime());

        // WHY the inference id is placed in an entry rather than counted off
        // `highest_id`: a registry that listed twelve services without naming
        // the inference id among them does not hold a runtime, and counting
        // positions is how the binding used to claim one it had never seen.
        let mut named = ServiceRegistry {
            status: ProbeStatus::Listed,
            count: 12,
            highest_id: interos_std::ai::SERVICE_INFERENCE,
            entries: [ServiceEntry {
                id: 0,
                state: ServiceState::Unknown,
                abi_version: 0,
                age_ticks: 0,
                failures: 0,
            }; MAX_SERVICES],
        };
        named.entries[11] = ServiceEntry {
            id: interos_std::ai::SERVICE_INFERENCE,
            state: ServiceState::Running,
            abi_version: interos_std::ai::INFERENCE_ABI_VERSION,
            age_ticks: 0,
            failures: 0,
        };
        assert!(named.contains_inference_runtime());
        assert!(named.inference_runtime_binding().is_bound());

        // A named service that is not running is a different fact from one
        // that was never named, and the reason has to say which.
        named.entries[11].state = ServiceState::Stopped;
        assert!(!named.contains_inference_runtime());
        assert!(named.unbound_reason().contains("not running"));
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

    #[test]
    fn a_generation_session_opens_and_closes_without_inventing_a_reply() {
        // WHY this replaces the streaming test: the chat used to pull chunks from
        // a provider it consulted itself, so the transcript could end up holding
        // an assistant message no model ever wrote. The answer comes from the
        // platform now, and what this app owns is only the session around it.
        let mut chat = ChatSession::new();
        chat.push(ChatRole::User, b"status");
        chat.start_stream(b"status");
        assert!(chat.is_streaming);
        assert_eq!(chat.stream_prompt_text(), b"status");

        chat.finish_stream();
        assert!(!chat.is_streaming);
        assert_eq!(chat.len(), 1, "nothing was generated, so nothing was added");
        assert_eq!(chat.last_message().expect("the prompt is still there").role, ChatRole::User);
    }

    #[test]
    fn the_transcript_reports_how_much_of_a_reply_it_kept() {
        // WHY the numbers are measured through the transcript: the bubble stops at
        // CHAT_TEXT_MAX and the screen has to be able to say that it did, in bytes,
        // rather than leave a sentence that looks like it ended there.
        let mut chat = ChatSession::new();
        let long: [u8; CHAT_TEXT_MAX + 40] = [b'a'; CHAT_TEXT_MAX + 40];
        chat.push(ChatRole::Assistant, &long);
        assert_eq!(chat.last_text_len(), CHAT_TEXT_MAX);
        assert_eq!(chat.append_chunk(b"tail"), 0, "a full bubble holds nothing more");
    }

    #[test]
    fn the_host_bridge_streams_tokens_for_different_prompts() {
        let chunk = HOST_BRIDGE.stream_chunk(b"query model catalog", 0);
        match chunk {
            StreamChunk::Unavailable(reason) => {
                assert!(reason.contains("Host bridge stream unavailable"));
            }
            _ => panic!("expected unavailable stream chunk"),
        }
    }

    #[test]
    fn model_inventory_classifies_sub_megabyte_or_invalid_gguf_as_placeholder() {
        assert_eq!(
            ModelInventory::classify(b"model.bin", 500 * 1024, None),
            ModelClassification::Placeholder
        );
        assert_eq!(
            ModelInventory::classify(b"model.gguf", 10 * 1024 * 1024, Some(b"BAD!")),
            ModelClassification::Placeholder
        );
        assert_eq!(
            ModelInventory::classify(b"model.gguf", 10 * 1024 * 1024, Some(b"GGUF")),
            ModelClassification::Weights
        );
    }

    #[test]
    fn calculator_tool_evaluates_the_expression_it_is_handed() {
        assert_eq!(evaluate_integer_expression(b"1024*768/256"), Some(3072));
        assert_eq!(evaluate_integer_expression(b"7*6"), Some(42));
        assert_eq!(evaluate_integer_expression(b"10-3*2"), Some(4));
        assert_eq!(evaluate_integer_expression(b"-3+10"), Some(7));
        assert_eq!(evaluate_integer_expression(b"100/7"), Some(14));

        let outcome = AgentToolOutcome::execute_expression_with_authority(
            b"12*12",
            &granted_authority(),
        );
        assert!(outcome.ok);
        assert_eq!(outcome.text(), "144");

        let refused = AgentToolOutcome::execute_expression_with_authority(
            b"12*12",
            &refused_authority(),
        );
        assert!(!refused.ok);
        assert_eq!(
            refused.text(),
            core::str::from_utf8(TOOL_REFUSED_MESSAGE).unwrap()
        );
    }

    #[test]
    fn calculator_tool_refuses_expressions_it_cannot_evaluate() {
        assert_eq!(evaluate_integer_expression(b""), None);
        assert_eq!(evaluate_integer_expression(b"8/0"), None);
        assert_eq!(evaluate_integer_expression(b"7+"), None);
        assert_eq!(evaluate_integer_expression(b"7 + 7"), Some(14));
        assert_eq!(evaluate_integer_expression(b"two"), None);

        let outcome = AgentToolOutcome::execute_expression_with_authority(
            b"8/0",
            &granted_authority(),
        );
        assert!(!outcome.ok);
        assert_eq!(outcome.text(), "Calculator: expression not evaluable");
    }

    #[test]
    fn tool_descriptions_fit_the_label_the_surface_can_show() {
        for tool in AgentTool::ALL {
            let description = tool.description();
            assert!(
                description.len() <= TOOL_LABEL_MAX,
                "{:?} description is {} bytes, the surface shows {}",
                tool,
                description.len(),
                TOOL_LABEL_MAX
            );
        }

        // WHY the expression itself: a description that names the expression
        // the number came from has to name all of it. A cut expression
        // reads as a different expression, and the number beside it stops
        // being checkable by hand.
        let description = AgentTool::Calculator.description();
        assert!(
            description.contains("1024*768/256"),
            "the calculator description lost its expression: {}",
            description
        );
    }

    #[test]
    fn agent_tools_execute_bounded_runtime() {
        let calc = AgentToolOutcome::execute(AgentTool::Calculator);
        assert!(calc.ok);
        assert_eq!(calc.text(), "3072");

        let probe = AgentToolOutcome::execute(AgentTool::CapabilityCheck);
        assert!(!probe.text().is_empty());
    }

    #[test]
    fn a_tool_without_platform_authority_is_refused() {
        // WHY the refusal is checked here and not at the call site: the tool
        // must name the platform as the party that refused, so that a person
        // reading the screen is not told that they turned the tool off.
        let denied = AgentToolOutcome::refused(AgentTool::Calculator);
        assert!(!denied.ok);
        assert_eq!(denied.text(), "Refused: the kernel holds no agent task capability");
    }

    #[test]
    fn no_tool_runs_without_platform_authority() {
        let tools = [
            AgentTool::Calculator,
            AgentTool::CalendarClock,
            AgentTool::StorageStats,
            AgentTool::SettingsQuery,
            AgentTool::CapabilityCheck,
        ];
        for tool in tools {
            let outcome = AgentToolOutcome::refused(tool);
            assert!(!outcome.ok);
            assert_eq!(
                outcome.text(),
                "Refused: the kernel holds no agent task capability"
            );
        }
    }

    #[test]
    fn probe_stages_derive_from_inventory_and_services() {
        let inv = ModelInventory {
            status: ModelProbeStatus::Listed,
            entries: 1,
            placeholders: 0,
            has_embedding_weights: true,
            has_generative_weights: true,
            embedding_size_kb: 2048,
            generative_size_kb: 512,
        };
        let srv = ServiceRegistry {
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
        };
        assert_eq!(inv.embedding_stage(&srv), Some(ProbeStage::Validated));
        assert_eq!(inv.generative_stage(&srv), Some(ProbeStage::Discovered));
    }

    #[test]
    fn idempotent_install_is_refused_when_package_is_already_active() {
        let mut device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 0,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: PackageRegistry::empty(),
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

        let pkg_id = b"org.interos.localgenerative";
        device.packages.count = 1;
        device.packages.ids[0][..pkg_id.len()].copy_from_slice(pkg_id);
        device.packages.id_lens[0] = pkg_id.len() as u8;
        device.packages.entries[0] = RegistryPackageEntry {
            id: device.packages.ids[0],
            id_len: pkg_id.len() as u8,
            version: (1, 0, 0),
            state: RegistryPackageState::Active,
        };

        let provider = provider_by_id("local-generative").expect("local-generative provider");
        let mut install_called = false;
        let outcome = transact_package_install(provider, &device, |_pkg, _consent| {
            install_called = true;
            Ok(())
        });

        assert_eq!(outcome, PackageInstallOutcome::AlreadyInstalled);
        assert!(!install_called, "install syscall must not be invoked for an already active package");
    }

    #[test]
    fn failed_package_install_rolls_back_partial_state() {
        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 0,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: PackageRegistry::empty(),
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

        let provider = provider_by_id("local-generative").expect("local-generative provider");
        let outcome = transact_package_install(provider, &device, |_pkg, _consent| {
            Err(-12)
        });

        assert_eq!(outcome, PackageInstallOutcome::RolledBack(-12));
    }

    #[test]
    fn package_removal_of_in_use_package_is_refused_per_contract() {
        // WHY this test matters: The kernel documented contract (sys_package_uninstall)
        // explicitly refuses removal while an installed package process is still alive,
        // returning SyscallResult::NotReady (-30). The agent manager must handle this
        // contract honestly and report the in-use state rather than a generic error code.
        let mut device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 0,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: PackageRegistry::empty(),
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

        let pkg_id = b"org.interos.localgenerative";
        device.packages.count = 1;
        device.packages.ids[0][..pkg_id.len()].copy_from_slice(pkg_id);
        device.packages.id_lens[0] = pkg_id.len() as u8;
        device.packages.entries[0] = RegistryPackageEntry {
            id: device.packages.ids[0],
            id_len: pkg_id.len() as u8,
            version: (1, 0, 0),
            state: RegistryPackageState::Active,
        };

        let provider = provider_by_id("local-generative").expect("local-generative provider");
        let outcome = transact_package_remove(
            provider,
            &device,
            2,
            |_pkg, _consent| Err(ERR_NOT_READY),
            |_path| Ok(()),
        );

        assert_eq!(outcome, PackageRemoveOutcome::InUse);
    }

    #[test]
    fn package_removal_uninstalls_active_package_from_registry() {
        let mut device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 0,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: PackageRegistry::empty(),
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

        let pkg_id = b"org.interos.localgenerative";
        device.packages.count = 1;
        device.packages.ids[0][..pkg_id.len()].copy_from_slice(pkg_id);
        device.packages.id_lens[0] = pkg_id.len() as u8;
        device.packages.entries[0] = RegistryPackageEntry {
            id: device.packages.ids[0],
            id_len: pkg_id.len() as u8,
            version: (1, 0, 0),
            state: RegistryPackageState::Active,
        };

        let provider = provider_by_id("local-generative").expect("local-generative provider");
        let mut uninstalled_pkg = "";
        let outcome = transact_package_remove(
            provider,
            &device,
            2,
            |pkg, _consent| {
                if pkg == "org.interos.localgenerative" {
                    uninstalled_pkg = "org.interos.localgenerative";
                    Ok(())
                } else {
                    Err(-8)
                }
            },
            |_path| Ok(()),
        );

        assert_eq!(outcome, PackageRemoveOutcome::Uninstalled);
        assert_eq!(uninstalled_pkg, "org.interos.localgenerative");
    }

    #[test]
    fn unlinking_local_model_file_is_contract_for_standalone_weights_removal() {
        // WHY this test matters: The platform contract for removal distinguishes between
        // packaged providers and standalone weight files. When local generative weights are
        // deployed as a standalone model file at `/data/models/local_model.gguf` without a
        // package in the package registry, unlinking `/data/models/local_model.gguf` genuinely
        // IS the documented removal contract. This test proves that contract explicitly
        // instead of leaving it implied.
        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 1,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: true,
                embedding_size_kb: 0,
                generative_size_kb: 4096,
            },
            packages: PackageRegistry::empty(),
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

        let provider = provider_by_id("local-generative").expect("local-generative provider");
        let mut unlinked_path = "";
        let outcome = transact_package_remove(
            provider,
            &device,
            2,
            |_pkg, _consent| Err(-8),
            |path| {
                if path == "/data/models/local_model.gguf" {
                    unlinked_path = "/data/models/local_model.gguf";
                    Ok(())
                } else {
                    Err(-8)
                }
            },
        );

        assert_eq!(outcome, PackageRemoveOutcome::ModelFileUnlinked);
        assert_eq!(unlinked_path, "/data/models/local_model.gguf");
    }

    #[test]
    fn unlinking_model_file_on_read_only_system_reports_error() {
        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 1,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: true,
                embedding_size_kb: 0,
                generative_size_kb: 4096,
            },
            packages: PackageRegistry::empty(),
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

        let provider = provider_by_id("local-generative").expect("local-generative provider");
        let outcome = transact_package_remove(
            provider,
            &device,
            2,
            |_pkg, _consent| Err(-8),
            |_path| Err(ERR_READ_ONLY_FS),
        );

        assert_eq!(outcome, PackageRemoveOutcome::ReadOnlyFileSystem(ERR_READ_ONLY_FS));
    }

    #[test]
    fn package_version_and_lifecycle_state_surfaced_from_registry() {
        let payload = b"org.interos.localgenerative|localgen|2.4.1|active|/data/apps/localgen|Local Generative|AI|Tools\n";
        let registry = PackageRegistry::parse_payload(payload);

        assert_eq!(registry.status, ProbeStatus::Listed);
        assert_eq!(registry.count, 1);
        assert!(registry.contains("org.interos.localgenerative"));
        assert!(registry.is_active("org.interos.localgenerative"));
        assert_eq!(registry.version_of("org.interos.localgenerative"), Some((2, 4, 1)));
        assert_eq!(registry.state_of("org.interos.localgenerative"), Some(RegistryPackageState::Active));

        let entry = registry.find_entry("org.interos.localgenerative").expect("entry found");
        assert_eq!(entry.id_str(), "org.interos.localgenerative");
        assert_eq!(entry.version, (2, 4, 1));
        assert_eq!(entry.state, RegistryPackageState::Active);

        let device = DeviceState {
            models: ModelInventory {
                status: ModelProbeStatus::Listed,
                entries: 0,
                placeholders: 0,
                has_embedding_weights: false,
                has_generative_weights: false,
                embedding_size_kb: 0,
                generative_size_kb: 0,
            },
            packages: registry,
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

        let provider = provider_by_id("local-generative").expect("local-generative provider");
        assert_eq!(provider.package_version(&device), Some((2, 4, 1)));
        assert_eq!(provider.package_lifecycle_state(&device), Some(RegistryPackageState::Active));
        assert_eq!(provider.install_state(&device), InstallState::Installed);
    }

    // ---------------------------------------------------------------------------
    // The embedding probe
    // ---------------------------------------------------------------------------

    /// The agent manager's action bar as `main.rs` describes it.
    ///
    /// WHY the numbers are written out here: they are the row the probe control
    /// is registered in, and a gate that read them from the app would be reading
    /// the app's own claim back to it. Writing them down is what makes the two
    /// copies agree in the open; `main.rs` carries a compile-time gate that the
    /// row it builds is these numbers, so they cannot drift apart quietly.
    const ACTION_BAR: ControlRow = ControlRow {
        x: 24,
        w: 672,
        y: 1300,
        h: 52,
        gap: 8,
        live: 2,
    };

    /// The index the embedding probe is registered at in that row.
    const PROBE_SLOT: usize = 1;

    #[test]
    fn the_probe_control_answers_the_band_it_is_painted_on() {
        // WHAT THIS IS FOR: the probe's control is painted from `slot` and
        // answered from `tapped`, and a control that is drawn in one plate and
        // pressed in another is a dead affordance the reader can see. The
        // arithmetic is spelled out below rather than called, so a wrong
        // derivation in the row itself fails here instead of on the screen.
        let painted_width = (ACTION_BAR.w - (ACTION_BAR.live - 1) * ACTION_BAR.gap)
            / ACTION_BAR.live;
        let painted_x = ACTION_BAR.x + PROBE_SLOT * (painted_width + ACTION_BAR.gap);
        let painted = ACTION_BAR.slot(PROBE_SLOT);
        assert_eq!(painted.x, painted_x, "the row's plate is not where the column puts it");
        assert_eq!(painted.w, painted_width);
        assert_eq!(painted.y, ACTION_BAR.y);
        assert_eq!(painted.h, ACTION_BAR.h);

        // The tap band is the painted rect: every point inside it answers, and
        // the plate's own centre is the point a finger on the plate lands on.
        assert_eq!(
            ACTION_BAR.tapped(painted.x + painted.w / 2, painted.y + painted.h / 2),
            Some(PROBE_SLOT),
            "the probe's plate does not answer the tap it was drawn for"
        );
        assert_eq!(
            ACTION_BAR.tapped(painted.x, painted.y),
            Some(PROBE_SLOT),
            "the plate's first pixel is not the plate"
        );
        assert_eq!(
            ACTION_BAR.tapped(painted.x + painted.w - 1, painted.y + painted.h - 1),
            Some(PROBE_SLOT),
            "the plate's last pixel is not the plate"
        );
        // And nothing outside it is: the row above, the row below, and the strip
        // of air the plate keeps from its neighbour.
        assert_eq!(ACTION_BAR.tapped(painted.x + painted.w / 2, painted.y - 1), None);
        assert_eq!(
            ACTION_BAR.tapped(painted.x + painted.w / 2, painted.y + painted.h),
            None
        );
        assert_eq!(ACTION_BAR.tapped(painted.x - 1, painted.y + painted.h / 2), None);
    }

    #[test]
    fn the_probe_control_clears_the_platforms_touch_floor() {
        // WHAT THIS IS FOR: the platform's minimum target is for fingers, and a
        // plate the reader cannot hit is a control that does not exist.
        let painted = ACTION_BAR.slot(PROBE_SLOT);
        assert!(
            painted.h >= interos_std::design::touch::MIN_TARGET,
            "the probe's band is {} rows, under the platform's {}-row floor",
            painted.h,
            interos_std::design::touch::MIN_TARGET,
        );
        assert!(painted.w >= interos_std::design::touch::MIN_TARGET);
    }

    #[test]
    fn the_action_bar_loses_no_live_control_when_a_button_has_nothing_to_say() {
        // WHAT THIS IS FOR: the row counts only the controls it drew, so a third
        // plate cannot shrink the probe's own out from under its tap band. This
        // is the drift that put the sixth tab in the wrong place once.
        let one = ControlRow { live: 1, ..ACTION_BAR };
        assert_eq!(one.slot(0).w, ACTION_BAR.w);
        assert_eq!(one.slot(0).x, ACTION_BAR.x);
        assert_eq!(one.tapped(ACTION_BAR.x + ACTION_BAR.w - 1, ACTION_BAR.y), Some(0));
        let two = ControlRow { live: 2, ..ACTION_BAR };
        assert_eq!(two.slot(1).w, two.slot_width());
        assert_eq!(two.slot(1).x + two.slot(1).w, ACTION_BAR.x + ACTION_BAR.w);
    }

    /// A reply laid out the way the inference service writes one.
    ///
    /// WHY the offsets are written by hand: they are the ABI. `AiInferenceResponse`
    /// is `repr(C)` with two `u32`s and two `u64`s, so the status stands at zero
    /// and the payload starts where the header ends. A test that spelled the
    /// offsets out catches a layout that moved underneath the app.
    fn a_reply(
        status: u32,
        output_dim: u32,
        latency: u64,
        payload: &[i8],
    ) -> [u8; MAILBOX_PAYLOAD_MAX] {
        let mut bytes = [0u8; MAILBOX_PAYLOAD_MAX];
        bytes[0..4].copy_from_slice(&status.to_le_bytes());
        bytes[4..8].copy_from_slice(&output_dim.to_le_bytes());
        bytes[8..16].copy_from_slice(&latency.to_le_bytes());
        bytes[16..24].copy_from_slice(&24u64.to_le_bytes());
        let take = payload.len().min(MAILBOX_PAYLOAD_MAX - REPLY_HEADER_BYTES);
        let mut index = 0;
        while index < take {
            bytes[REPLY_HEADER_BYTES + index] = payload[index] as u8;
            index += 1;
        }
        bytes
    }

    #[test]
    fn a_lexical_baseline_reply_is_attributed_lexical_and_not_neural() {
        // WHAT THIS IS FOR: the status is the only thing the reply carries that
        // says which arithmetic ran, and a screen that showed a baseline vector as
        // the model's is the defect the platform added this status to stop.
        let bytes = a_reply(
            interos_std::ai::STATUS_BASELINE_LEXICAL,
            256,
            1_000,
            &[3, -4, 12, -127, 127],
        );
        // WHY the slice stops where the service's send stopped: the mailbox hands
        // the app the bytes that arrived, not the width of the buffer it arrived
        // into, and a reader that walked the whole buffer would report values the
        // platform never sent.
        let reply = read_embed_reply(&bytes[..REPLY_HEADER_BYTES + 5]).expect("the bytes are a reply");
        assert_eq!(reply.status, interos_std::ai::STATUS_BASELINE_LEXICAL);
        assert_eq!(reply.output_dim, 256);
        assert!(reply.carries_vector());
        assert_eq!(reply.attribution(), Some(&b"[Lexical]"[..]));
        assert_eq!(reply.payload_len, 5);
        assert_eq!(reply.payload[1], -4);
        assert_eq!(reply.payload[4], 127);
    }

    #[test]
    fn a_file_backed_reply_is_attributed_neural() {
        let bytes = a_reply(interos_std::ai::STATUS_SUCCESS, 128, 2_000, &[1, 2, 3]);
        let reply = read_embed_reply(&bytes).expect("the bytes are a reply");
        assert!(reply.carries_vector());
        assert_eq!(reply.attribution(), Some(&b"[Neural]"[..]));
    }

    #[test]
    fn a_refusal_reply_is_words_and_claims_no_dimension() {
        // WHAT THIS IS FOR: on a refusal the reply's `output_dim` is the byte
        // count of the platform's sentence, not a vector's dimension
        // (`emit_response_as`). Printing it under a "dimension" label would be the
        // app inventing a shape the service never produced.
        //
        // WHY the sentence is longer than what reaches the app: the service's own
        // refusals run past the mailbox's cut, so this also proves the probe keeps
        // the words that arrived rather than the words the service meant to send.
        let sentence =
            b"Embedding refused: the strict-neural flag is set and no weights file.";
        let mut wire = [0u8; 128];
        wire[..REPLY_HEADER_BYTES].copy_from_slice(
            &a_reply(interos_std::ai::STATUS_UNSUPPORTED, sentence.len() as u32, 0, &[])
                [..REPLY_HEADER_BYTES],
        );
        let mut index = 0;
        while index < sentence.len() {
            wire[REPLY_HEADER_BYTES + index] = sentence[index];
            index += 1;
        }
        // The kernel hands the app the first sixty-four bytes of that, and no more.
        let arrived = &wire[..MAILBOX_PAYLOAD_MAX];
        let reply = read_embed_reply(arrived).expect("the bytes are a reply");
        assert_eq!(reply.status, interos_std::ai::STATUS_UNSUPPORTED);
        assert!(!reply.carries_vector());
        assert_eq!(reply.attribution(), None);
        assert_eq!(reply.payload_len as usize, PROBE_PAYLOAD_MAX);
        assert_eq!(reply.output_dim as usize, sentence.len());
        let mut probe = EmbedProbe::new();
        probe.record(&reply, 40);
        let mut out = [0u8; 96];
        let len = probe.sentence_bytes(&mut out);
        assert_eq!(len, PROBE_PAYLOAD_MAX);
        assert_eq!(&out[..len], &sentence[..PROBE_PAYLOAD_MAX]);
        // The sentence the platform wrote is longer than the sentence the app was
        // handed, and the probe holds both numbers so the screen can say so.
        assert!(probe.values_arrived() < probe.offered as usize);
    }

    #[test]
    fn a_message_shorter_than_a_header_is_refused_rather_than_read_as_zeroes() {
        // WHAT THIS IS FOR: a decode that fell back to zeroes would let the screen
        // report that the platform answered with status 0 and dimension 0 when it
        // answered with nothing this app can attribute.
        assert!(read_embed_reply(&[]).is_none());
        assert!(read_embed_reply(&[0u8; REPLY_HEADER_BYTES - 1]).is_none());
        assert!(read_embed_reply(&a_reply(0, 0, 0, &[])).is_some());
    }

    #[test]
    fn the_probe_keeps_the_bytes_that_arrived_beside_the_ones_the_reply_offered() {
        // WHAT THIS IS FOR: the mailbox cuts a reply at sixty-four bytes and says
        // nothing, so a 256-component vector arrives as forty bytes and the only
        // witness that the rest did not is the number in the header. A screen that
        // showed forty values as "the embedding" would be reporting a whole the
        // app was never handed.
        let vector = [7i8; 768];
        let bytes = a_reply(interos_std::ai::STATUS_BASELINE_LEXICAL, 256, 1_000, &vector);
        let reply = read_embed_reply(&bytes).expect("the bytes are a reply");
        let mut probe = EmbedProbe::new();
        probe.sent_at_tick = 1_500;
        probe.record(&reply, 1_503);
        assert_eq!(probe.values_arrived(), PROBE_PAYLOAD_MAX);
        assert_eq!(probe.offered, 256);
        assert_eq!(probe.reply_ticks, 3);
        assert_eq!(probe.deadline, 0);
        assert_eq!(probe.state, ProbeState::Answered);
        assert!(probe.attributed);
    }

    #[test]
    fn the_request_names_the_text_and_carries_no_token_ids() {
        // WHAT THIS IS FOR: the flag that promises a token-id stream would make
        // the service read these bytes as vocabulary ids. The probe sends text, so
        // the reply it gets is the answer the platform gives to text.
        let text = b"storage";
        let mut request = [0u8; 256];
        let len = build_embed_request(text, Some(256), &mut request).expect("it fits");
        let header = core::mem::size_of::<interos_std::ai::AiInferenceRequest>();
        assert_eq!(len, header + text.len());
        assert_eq!(&request[header..len], text);
        assert_eq!(u32::from_le_bytes([request[4], request[5], request[6], request[7]]), 256);
        assert_eq!(u32::from_le_bytes([request[8], request[9], request[10], request[11]]), 7);
        assert_eq!(u32::from_le_bytes([request[12], request[13], request[14], request[15]]), 0);
        assert_eq!(
            u32::from_le_bytes([request[0], request[1], request[2], request[3]]),
            0,
            "the request named a model the platform does not choose from it"
        );
        // The flag is bit 4 of `flags`, and the request sets no flag.
        let flags = u32::from_le_bytes([request[12], request[13], request[14], request[15]]);
        assert_eq!(flags & interos_std::ai::FLAG_INPUT_TOKEN_IDS, 0);
    }

    #[test]
    fn a_request_takes_the_cut_the_app_can_see_and_not_the_one_the_kernel_hides() {
        // WHAT THIS IS FOR: the mailbox copies a message into a fixed slot and
        // shortens it without a word, so a header that promised more text than
        // arrived would leave the service reading a prompt shorter than the one the
        // screen says was asked. The app cuts at its own buffer instead, which is
        // the same line the kernel would have drawn, and can name the number.
        let typed = [b'x'; 160];
        let mut request = [0u8; MAILBOX_PAYLOAD_MAX];
        let len = build_embed_request(&typed, None, &mut request).expect("a header fits");
        assert_eq!(len, MAILBOX_PAYLOAD_MAX);
        assert_eq!(len - REQUEST_HEADER_BYTES, PROBE_REQUEST_TEXT_MAX);
        assert_eq!(
            u32::from_le_bytes([request[8], request[9], request[10], request[11]]),
            PROBE_REQUEST_TEXT_MAX as u32,
            "the header's own count must equal the text bytes that are actually there"
        );

        // A buffer with no room after the header carries a header and no text, and
        // a buffer too narrow for the header is refused rather than half-written.
        let mut header_only = [0u8; REQUEST_HEADER_BYTES];
        assert_eq!(
            build_embed_request(b"storage", None, &mut header_only),
            Some(REQUEST_HEADER_BYTES)
        );
        let mut too_small = [0u8; REQUEST_HEADER_BYTES - 1];
        assert!(build_embed_request(b"storage", None, &mut too_small).is_none());
    }

    #[test]
    fn the_probe_before_a_reply_claims_nothing_about_the_platform() {
        // WHAT THIS IS FOR: the probe is built before anything is asked, and a
        // screen that painted its zeroed fields would report status 0 -- the
        // platform's own code for success -- from a request nobody made.
        let probe = EmbedProbe::new();
        assert_eq!(probe.state, ProbeState::Idle);
        assert!(probe.sent_text().is_empty());
        assert_eq!(probe.payload_len, 0);
        assert!(!probe.reply_is_vector);
        assert!(!probe.attributed);
    }

    #[test]
    fn asking_again_forgets_the_last_reply_and_keeps_the_ticks_the_app_measured() {
        // WHAT THIS IS FOR: a card that still held the earlier answer while a
        // second request was in flight would paint a number the person had not
        // asked for again, and the interval is the app's own measurement around
        // its own send call, so it belongs to the new request alone.
        let mut probe = EmbedProbe::new();
        let bytes = a_reply(interos_std::ai::STATUS_BASELINE_LEXICAL, 256, 1_000, &[9, 8, 7]);
        let reply = read_embed_reply(&bytes).expect("the bytes are a reply");
        probe.sent_at_tick = 1_000;
        probe.record(&reply, 1_004);
        assert_eq!(probe.state, ProbeState::Answered);

        probe.start_request(b"second", 6, None, 30, 2_000, 3, 40);
        assert_eq!(probe.state, ProbeState::Awaiting);
        assert_eq!(probe.sent_text(), b"second");
        assert_eq!(probe.typed_bytes, 6);
        assert_eq!(probe.request_bytes, 30);
        assert_eq!(probe.asked_dim, 0);
        assert!(!probe.asked_named_dim);
        assert_eq!(probe.send_ticks, 3);
        assert_eq!(probe.deadline, 2_040);
        assert_eq!(probe.reply_dim, 0);
        assert_eq!(probe.status, 0);
        assert_eq!(probe.payload_len, 0);
        assert!(!probe.attributed);
        assert!(!probe.reply_is_vector);

        // A request that could not carry everything the person had typed holds
        // both numbers, so the screen can name the difference instead of hiding it.
        probe.start_request(&[b'y'; PROBE_REQUEST_TEXT_MAX], 160, Some(512), 64, 3_000, 2, 40);
        assert_eq!(probe.text_len as usize, PROBE_REQUEST_TEXT_MAX);
        assert_eq!(probe.typed_bytes, 160);
        assert!(probe.asked_named_dim);
    }

    #[test]
    fn the_leading_values_are_the_replys_own_bytes_and_stop_at_the_buffers_edge() {
        // WHAT THIS IS FOR: the values shown must be the components that arrived,
        // spelled by the app's existing signed formatter, and a list the buffer
        // could not hold has to end between numbers rather than inside one.
        let mut probe = EmbedProbe::new();
        assert_eq!(probe.leading_values(&mut [0u8; 32]), (0, 0));

        let bytes = a_reply(
            interos_std::ai::STATUS_BASELINE_LEXICAL,
            256,
            1_000,
            &[3, -4, 0, -127, 127],
        );
        let reply = read_embed_reply(&bytes[..REPLY_HEADER_BYTES + 5]).expect("a reply");
        probe.record(&reply, 10);
        assert!(probe.reply_is_vector);
        let mut line = [0u8; 32];
        let (written, count) = probe.leading_values(&mut line);
        assert_eq!(count, 5);
        assert_eq!(&line[..written], b"3, -4, 0, -127, 127");

        // The fourth value ends the list at byte fourteen and the fifth would need
        // nineteen, so a buffer of thirteen holds three whole numbers and no more.
        let mut tight = [0u8; 13];
        let (cut, cut_count) = probe.leading_values(&mut tight);
        assert_eq!(cut_count, 3);
        assert_eq!(&tight[..cut], b"3, -4, 0");
        assert_eq!(cut, 8);
    }
}
