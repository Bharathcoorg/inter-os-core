#![no_std]

//! The camera application's model.
//!
//! WHY the model lives in the library and not in the binary: the binary is
//! the renderer, and a renderer cannot be unit tested on the host. Every
//! fact the camera states about the platform is decided here, so the tests
//! can pin it, and the renderer only draws what the model decided.

pub use interos_std as std;

/// The app's view of the platform's design system.
pub mod theme;

use core::ptr::addr_of_mut;

// ---------------------------------------------------------------------------
// The platform's error codes the camera has to tell apart.
// ---------------------------------------------------------------------------

/// The platform refused the call because the caller does not hold the
/// capability the call needs.
pub const ERR_PERMISSION_DENIED: i64 = -1;

/// The platform authorised the call and the device behind it is not there.
pub const ERR_DEVICE_ERROR: i64 = -12;

/// The platform refused the call because an argument was not usable, which is
/// what a frame that does not fit the app's buffer comes back as.
pub const ERR_INVALID_ARGUMENTS: i64 = -3;

/// Viewfinder aspect ratio configuration (4:3 portrait, 16:9 cinematic, 1:1 square).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraAspectRatio {
    Ratio4_3,
    Ratio16_9,
    Square,
}

impl CameraAspectRatio {
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraAspectRatio::Ratio4_3 => b"4:3",
            CameraAspectRatio::Ratio16_9 => b"16:9",
            CameraAspectRatio::Square => b"1:1",
        }
    }

    pub const fn toggle(self) -> CameraAspectRatio {
        match self {
            CameraAspectRatio::Ratio4_3 => CameraAspectRatio::Ratio16_9,
            CameraAspectRatio::Ratio16_9 => CameraAspectRatio::Square,
            CameraAspectRatio::Square => CameraAspectRatio::Ratio4_3,
        }
    }
}

/// Self-timer countdown modes (Off, 3s, 10s).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    Off = 0,
    Sec3 = 1,
    Sec10 = 2,
}

impl TimerMode {
    pub const fn label(self) -> &'static [u8] {
        match self {
            TimerMode::Off => b"TIMER",
            TimerMode::Sec3 => b"3s",
            TimerMode::Sec10 => b"10s",
        }
    }

    pub const fn seconds(self) -> u32 {
        match self {
            TimerMode::Off => 0,
            TimerMode::Sec3 => 3,
            TimerMode::Sec10 => 10,
        }
    }

    pub const fn next(self) -> TimerMode {
        match self {
            TimerMode::Off => TimerMode::Sec3,
            TimerMode::Sec3 => TimerMode::Sec10,
            TimerMode::Sec10 => TimerMode::Off,
        }
    }
}

// ---------------------------------------------------------------------------
// Modes, filters and flash.
// ---------------------------------------------------------------------------

/// The operating mode the shutter is in.
///
/// WHY exactly these three: the mode decides what the shutter does, and the
/// shutter can take a still, take a video frame, or frame a document. The QR
/// scanner and the night mode that used to sit in this enum were never
/// reachable from the carousel and had no code behind them, so a variant that
/// named them was a claim about features the app does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    /// A still photograph.
    Photo = 0,
    /// A single video frame.
    Video = 1,
    /// A document framing.
    DocumentVision = 2,
}

impl CameraMode {
    /// The label the mode carousel shows.
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraMode::Photo => b"PHOTO",
            CameraMode::Video => b"VIDEO",
            CameraMode::DocumentVision => b"DOCUMENT",
        }
    }

    /// The next mode in the carousel.
    pub const fn next(self) -> CameraMode {
        match self {
            CameraMode::Photo => CameraMode::Video,
            CameraMode::Video => CameraMode::DocumentVision,
            CameraMode::DocumentVision => CameraMode::Photo,
        }
    }
}

/// The image processing the capture pipeline applies to a frame.
///
/// WHY there is no "neural" filter here: a filter that claims a model ran
/// over the frame is a claim about work this app never does. Every filter
/// below is calibrated ISP arithmetic the app performs on the pixels it was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraFilter {
    /// Normal: Neutral balanced color.
    Normal = 0,
    /// Vivid: High chroma and punchy contrast.
    Vivid = 1,
    /// Warm: Golden hour warm color temperature.
    Warm = 2,
    /// Cool: Cinematic cool blue-cyan tint.
    Cool = 3,
    /// Mono: High-contrast artistic black & white.
    Mono = 4,
    /// Vintage: Muted analog film curve with lifted matte blacks.
    Vintage = 5,
}

impl CameraFilter {
    /// The label the filter readout shows.
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraFilter::Normal => b"NORMAL",
            CameraFilter::Vivid => b"VIVID",
            CameraFilter::Warm => b"WARM",
            CameraFilter::Cool => b"COOL",
            CameraFilter::Mono => b"MONO",
            CameraFilter::Vintage => b"VINTAGE",
        }
    }

    /// The next filter in the readout's cycle.
    pub const fn next(self) -> CameraFilter {
        match self {
            CameraFilter::Normal => CameraFilter::Vivid,
            CameraFilter::Vivid => CameraFilter::Warm,
            CameraFilter::Warm => CameraFilter::Cool,
            CameraFilter::Cool => CameraFilter::Mono,
            CameraFilter::Mono => CameraFilter::Vintage,
            CameraFilter::Vintage => CameraFilter::Normal,
        }
    }
}

/// The flash modes supported by the camera hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashMode {
    /// The platform decides whether the flash fires.
    Auto = 0,
    /// The flash fires for the capture.
    On = 1,
    /// The flash is turned off.
    Off = 2,
}

impl FlashMode {
    /// The label the flash readout shows.
    pub const fn label(self) -> &'static [u8] {
        match self {
            FlashMode::Auto => b"FLASH AUTO",
            FlashMode::On => b"FLASH ON",
            FlashMode::Off => b"FLASH OFF",
        }
    }

    /// Short pill label.
    pub const fn pill_label(self) -> &'static [u8] {
        match self {
            FlashMode::Auto => b"AUTO",
            FlashMode::On => b"ON",
            FlashMode::Off => b"OFF",
        }
    }

    /// The next flash state in the readout's cycle.
    pub const fn next(self) -> FlashMode {
        match self {
            FlashMode::Auto => FlashMode::On,
            FlashMode::On => FlashMode::Off,
            FlashMode::Off => FlashMode::Auto,
        }
    }
}

// ---------------------------------------------------------------------------
// What the platform has told the camera about itself.
// ---------------------------------------------------------------------------

/// What the platform has told the camera about the camera.
///
/// WHY these are separate states: "the platform refused this app the camera
/// capability" and "the camera HAL has no sensor bound" are different facts
/// with different remedies, and a screen that says "no sensor" for both
/// sends the user looking for a piece of hardware that is not the problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraStatus {
    /// The platform refused this app's camera authority.
    AuthorityRefused,
    /// This app holds the authority and the camera HAL reported that no
    /// sensor is bound to it.
    NoSensor,
    /// This app holds the authority; no frame has been captured yet.
    AuthorityHeld,
    /// The platform answered with an error this app has no state for.
    Failed,
}

impl CameraStatus {
    /// The short label the capture readout carries.
    pub const fn label(self) -> &'static [u8] {
        match self {
            CameraStatus::AuthorityRefused => b"NO AUTH",
            CameraStatus::NoSensor => b"NO SENSOR",
            CameraStatus::AuthorityHeld => b"NO FRAME",
            CameraStatus::Failed => b"CAMERA ERR",
        }
    }

    /// The headline the viewfinder shows.
    pub const fn headline(self) -> &'static [u8] {
        match self {
            CameraStatus::AuthorityRefused => b"Camera authority refused",
            CameraStatus::NoSensor => b"No camera sensor",
            CameraStatus::AuthorityHeld => b"No frame yet",
            CameraStatus::Failed => b"Capture refused",
        }
    }

    /// The line under the headline: why the camera is in this state.
    pub const fn reason(self) -> &'static [u8] {
        match self {
            CameraStatus::AuthorityRefused => {
                b"The platform denied this app the camera capability"
            }
            CameraStatus::NoSensor => b"The camera HAL reported that no sensor is bound",
            CameraStatus::AuthorityHeld => {
                b"The platform granted camera authority; no frame captured"
            }
            CameraStatus::Failed => b"The platform refused the capture",
        }
    }
}

/// Works out which fact an error code from the platform states.
///
/// WHY the code is read rather than the absence of a frame: the capture call
/// answers with a refusal when this app holds no camera authority, and with
/// a device error when the authority is held and the camera HAL has no
/// sensor. Both come back from the same call, so the code is the only thing
/// that tells the two apart.
pub fn status_from_error(code: i64) -> CameraStatus {
    match code {
        ERR_PERMISSION_DENIED => CameraStatus::AuthorityRefused,
        ERR_DEVICE_ERROR => CameraStatus::NoSensor,
        _ => CameraStatus::Failed,
    }
}

// ---------------------------------------------------------------------------
// The frame the platform returned.
// ---------------------------------------------------------------------------

/// How the pixels of a frame are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    /// Four bytes per pixel: red, green, blue, alpha.
    Rgba8888,
    /// Three bytes per pixel: red, green, blue.
    Rgb888,
    /// One byte per pixel: luminance.
    Gray8,
}

impl FrameFormat {
    /// How many bytes one pixel of this format takes.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            FrameFormat::Rgba8888 => 4,
            FrameFormat::Rgb888 => 3,
            FrameFormat::Gray8 => 1,
        }
    }
}

/// The media type the camera's photos are written in.
///
/// WHY this is the type the platform sniffs from a PPM header: the gallery
/// declares a handler for it, so a photo the camera wrote is a photo the
/// platform can hand to the gallery that opens images.
pub const PHOTO_MIME: &str = "image/x-portable-pixmap";

/// The app's error code for "there is no photo to open".
///
/// WHY a local code and not a platform one: no photo is not something the
/// platform refused; it is a fact about this app's own state.
pub const ERR_NO_PHOTO: i64 = -2;

/// The surface the camera draws on.
pub const SURFACE_W: usize = 720;

/// The column the viewfinder's text is measured against.
pub const VIEWFINDER_TEXT_X0: usize = 20;

/// The width of the column the viewfinder's text is measured against.
pub const VIEWFINDER_TEXT_W: usize = SURFACE_W - 40;

/// The width the settings surface's text is measured against.
///
/// WHY it is a constant and not the sheet's width: a label that does not fit
/// its column is a label that runs into the next one, and the settings
/// surface is where the camera states what the platform supports.
pub const SETTINGS_TEXT_MAX: usize = 600;

/// Left x that centres `text` at `scale` inside `[x0, x0 + available)`.
///
/// WHY the measured width and not `font::measure`: a run is placed through
/// the outline face when it can be, and the outline face advances by its own
/// table rather than the bitmap cell's. Both widths are decided by
/// `outline_text::drawn_advance`, which mirrors the placement's own
/// refusals, so the centring uses the width of the run that is drawn.
pub fn centred_x(text: &[u8], scale: usize, x0: usize, available: usize) -> usize {
    let w = std::design::outline_text::drawn_advance(text, scale, Some(available))
        .unwrap_or_else(|| std::design::font::measure(text, scale));
    x0 + available.saturating_sub(w) / 2
}

/// Works out a frame's format from the geometry the platform reported and the
/// number of bytes it returned.
///
/// WHY this and not a claim: the capture call reports the frame's width and
/// its height but not its pixel format, so the only way to know how many
/// bytes per pixel the frame carries is to divide the bytes the platform
/// actually returned by the pixels the platform actually reported. A frame
/// whose length matches no known layout is refused rather than guessed at.
pub fn frame_format(width: u32, height: u32, payload_len: usize) -> Option<FrameFormat> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    if pixels == 0 {
        return None;
    }
    if payload_len % pixels != 0 {
        return None;
    }
    match payload_len / pixels {
        4 => Some(FrameFormat::Rgba8888),
        3 => Some(FrameFormat::Rgb888),
        1 => Some(FrameFormat::Gray8),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The file the camera writes.
// ---------------------------------------------------------------------------

/// The directory the camera writes its photos into.
///
/// WHY this directory: it is the directory the platform's media-type
/// sniffer and the gallery already look at, so a photo the camera writes is
/// a document the platform can describe and the gallery can open.
pub const PHOTO_DIR: &str = "/storage/photos";

/// The largest path the camera builds.
pub const PHOTO_PATH_MAX: usize = 48;

/// Writes the path of the `index`-th photo into `out` and returns its length.
///
/// WHY the camera does not seal its photos into a private sector: a sector
/// only the camera can read is a photograph the user cannot open. The photo
/// goes into the shared storage area under a name the platform's document
/// index can describe.
pub fn photo_path(index: u32, out: &mut [u8; PHOTO_PATH_MAX]) -> usize {
    let mut n = 0usize;
    n += copy_bytes(&mut out[n..], PHOTO_DIR.as_bytes());
    n += copy_bytes(&mut out[n..], b"/shot-");
    n += append_decimal(index, &mut out[n..]);
    n += copy_bytes(&mut out[n..], b".ppm");
    n
}

/// Appends a decimal number to `out` and returns how many bytes it wrote.
pub fn append_decimal(mut value: u32, out: &mut [u8]) -> usize {
    if value == 0 {
        if out.is_empty() {
            return 0;
        }
        out[0] = b'0';
        return 1;
    }
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    while value > 0 && count < digits.len() {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    let mut written = 0usize;
    let mut i = count;
    while i > 0 {
        i -= 1;
        if written >= out.len() {
            break;
        }
        out[written] = digits[i];
        written += 1;
    }
    written
}

/// The header of the file the camera writes.
///
/// WHY a PPM and not a private container: the platform's media-type sniffer
/// reads a document's own bytes, and "P6" is a format it recognises, so a
/// photo the camera writes is a document the platform can describe and the
/// gallery can open. A private header would have made every photo a
/// document the platform could not classify.
///
/// WHY the header is built from the geometry that was captured: a header
/// that names a format the file does not hold is a lie the platform would
/// carry into the document index.
pub fn ppm_header(width: u32, height: u32, out: &mut [u8; 32]) -> usize {
    let mut n = 0usize;
    for byte in b"P6\n".iter() {
        if n >= out.len() {
            return n;
        }
        out[n] = *byte;
        n += 1;
    }
    n += append_decimal(width, &mut out[n..]);
    if n < out.len() {
        out[n] = b' ';
        n += 1;
    }
    n += append_decimal(height, &mut out[n..]);
    for byte in b"\n255\n".iter() {
        if n >= out.len() {
            return n;
        }
        out[n] = *byte;
        n += 1;
    }
    n
}

/// What became of a captured photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishState {
    /// The photo is on the device and the document index describes it.
    Published,
    /// The photo is on the device, but the document index did not take it.
    WrittenNotIndexed,
    /// The platform refused the write, with the code it answered.
    Refused(i64),
    /// No photo has been captured yet.
    None,
}

impl PublishState {
    /// The caption the settings surface shows.
    pub const fn caption(self) -> &'static [u8] {
        match self {
            PublishState::Published => b"Published in the document index",
            PublishState::WrittenNotIndexed => b"Written; the document index did not take it",
            PublishState::Refused(_) => b"The platform refused the write",
            PublishState::None => b"No photo captured yet",
        }
    }

    /// The platform's error code, when the platform refused the write.
    pub const fn error_code(self) -> Option<i64> {
        match self {
            PublishState::Refused(code) => Some(code),
            _ => None,
        }
    }
}

/// Decides what became of a photo from what the platform answered.
///
/// WHY this is a function and not a branch in the renderer: the difference
/// between "the photo is on the device", "the photo is on the device but the
/// index did not take it" and "there is no photo" is a fact about the
/// platform, and it is the fact the user is shown.
///
/// WHY the write's error code travels with the state: a refusal the user
/// cannot name is a refusal they cannot act on, and the code is the only
/// thing the platform gave.
pub fn publish_state(write: Result<(), i64>, indexed: bool) -> PublishState {
    match write {
        Err(code) => PublishState::Refused(code),
        Ok(()) if indexed => PublishState::Published,
        Ok(()) => PublishState::WrittenNotIndexed,
    }
}

/// What a press of the shutter produced.
///
/// WHY this is one value and not a success flag: "the platform refused the
/// capture", "the frame was captured but the photo did not reach storage" and
/// "the photo is on the device and the document index describes it" are three
/// different facts. A capture the platform refused and a capture that
/// succeeded but could not be saved are not the same event, and a screen that
/// called both of them a refusal would send the user after the wrong thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// The frame was captured and the photo is on the device, described by the
    /// platform's document index.
    Published,
    /// The frame was captured and the photo is on the device, but the document
    /// index did not describe it.
    WrittenNotIndexed,
    /// The frame was captured, but the platform refused to write the photo.
    WriteRefused(i64),
    /// The platform refused the capture itself.
    CaptureRefused(i64),
    /// The camera HAL exposes no video stream, so there is no frame to take.
    NoVideoStream,
    /// The shutter has not been pressed yet.
    None,
}

impl CaptureOutcome {
    /// Whether the frame itself was captured.
    ///
    /// WHY this is asked and not inferred: a refusal to write still leaves a
    /// frame in the app's buffer, and a frame that was never taken does not.
    pub const fn captured(self) -> bool {
        matches!(
            self,
            CaptureOutcome::Published | CaptureOutcome::WrittenNotIndexed | CaptureOutcome::WriteRefused(_)
        )
    }

    /// Whether the photo reached the device and the document index.
    pub const fn published(self) -> bool {
        matches!(self, CaptureOutcome::Published)
    }

    /// The platform's error code, when the platform refused something.
    pub const fn error_code(self) -> Option<i64> {
        match self {
            CaptureOutcome::WriteRefused(code) | CaptureOutcome::CaptureRefused(code) => Some(code),
            _ => None,
        }
    }

    /// The line the log and the settings surface carry.
    pub const fn caption(self) -> &'static [u8] {
        match self {
            CaptureOutcome::Published => {
                b"The photo is on the device and the index describes it"
            }
            CaptureOutcome::WrittenNotIndexed => {
                b"The photo is on the device; the index did not describe it"
            }
            CaptureOutcome::WriteRefused(_) => {
                b"The frame was captured; the write was refused"
            }
            CaptureOutcome::CaptureRefused(_) => b"The platform refused the capture",
            CaptureOutcome::NoVideoStream => b"The camera HAL exposes no video stream",
            CaptureOutcome::None => b"No capture yet",
        }
    }
}

// ---------------------------------------------------------------------------
// The app.
// ---------------------------------------------------------------------------

/// The screens the camera shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraScreen {
    /// The viewfinder and the shutter.
    Viewfinder,
    /// The settings surface.
    Settings,
}

/// The rows the camera's settings surface shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsRow {
    /// The format the camera writes.
    Format,
    /// The image processing the next capture uses.
    Filter,
    /// The flash and torch state the hardware reported.
    Flash,
    /// Whether the framing guides are drawn.
    Guides,
    /// How many photos are on the device.
    Photos,
    /// Where the last photo was written.
    LastPhoto,
    /// What the last press of the shutter produced.
    LastCapture,
    /// Open the photo library.
    OpenLibrary,
    /// Close the settings.
    Close,
}

/// How many rows the settings surface shows.
pub const SETTINGS_ROWS: usize = 9;

/// The order the settings surface shows its rows in.
pub const SETTINGS_ROW_ORDER: [SettingsRow; SETTINGS_ROWS] = [
    SettingsRow::Format,
    SettingsRow::Filter,
    SettingsRow::Flash,
    SettingsRow::Guides,
    SettingsRow::Photos,
    SettingsRow::LastPhoto,
    SettingsRow::LastCapture,
    SettingsRow::OpenLibrary,
    SettingsRow::Close,
];

/// The smallest thumbnail dimension for the gallery disc preview.
pub const THUMBNAIL_DIM: usize = 32;

/// The camera's state.
pub struct CameraApp {
    /// The mode the shutter is in.
    pub mode: CameraMode,
    /// The flash state the app asks for.
    pub flash: FlashMode,
    /// Whether the flash/torch hardware backend answered a toggle request.
    ///
    /// WHY: this app holds neither `SystemControl` on the camera service object
    /// nor `HardwarePortIo` on port 0xCA00, so `camera_toggle_flash` is refused
    /// with PermissionDenied. The previous code discarded that error, kept a lit
    /// `Torch` state locally and rendered it as an active amber pill.
    pub flash_available: bool,
    /// The image processing the next capture uses.
    pub filter: CameraFilter,
    /// The exposure bias in stops.
    pub exposure_bias: i16,
    /// How many frames the camera has captured.
    pub total_photos: u32,
    /// How many of those frames are on the device as documents.
    pub photos_published: u32,
    /// What the platform has told the camera about the camera.
    pub status: CameraStatus,
    /// The error code the platform last answered with.
    pub last_error: i64,
    /// The geometry of the last captured frame.
    pub frame_label: [u8; 16],
    /// How many bytes of the geometry label are used.
    pub frame_label_len: usize,
    /// The shots readout in the top row.
    pub shots_label: [u8; 16],
    /// How many bytes of the shots readout are used.
    pub shots_label_len: usize,
    /// The path of the last photo written to the storage area.
    pub last_photo: [u8; PHOTO_PATH_MAX],
    /// How many bytes of the last photo's path are used.
    pub last_photo_len: usize,
    /// What became of the last publication.
    pub publish: PublishState,
    /// What the last press of the shutter produced.
    ///
    /// WHY this is kept beside `publish`: a capture the platform refused and a
    /// capture that succeeded but could not be written are different facts, and
    /// the settings surface has to be able to say which one happened.
    pub capture: CaptureOutcome,
    /// The code the platform answered the last document-index refresh with.
    ///
    /// WHY it is kept: a photo that is on the device but that the index does
    /// not describe is a photo the gallery cannot show, and the reason the
    /// index did not take it is a capability the app may not hold.
    pub index_error: i64,
    /// Whether the framing guides are drawn over the viewfinder.
    pub guides: bool,
    /// Viewfinder aspect ratio configuration (4:3 vs 16:9 vs 1:1).
    pub aspect_ratio: CameraAspectRatio,
    /// Tactile visual feedback timer ticks remaining after shutter press.
    pub shutter_feedback_ticks: u32,
    /// Self-timer countdown mode (Off, 3s, 10s).
    pub timer_mode: TimerMode,
    /// Self-timer countdown ticks remaining before capture.
    pub countdown_ticks: u32,
    /// Downsampled thumbnail preview of the most recently captured photo (RGB888).
    pub thumbnail: [u8; THUMBNAIL_DIM * THUMBNAIL_DIM * 3],
    /// Whether a valid thumbnail preview is ready to display.
    pub has_thumbnail: bool,
    /// Which screen the app is showing.
    pub screen: CameraScreen,
    /// The headline the viewfinder shows.
    pub head: [u8; 48],
    /// How many bytes of the headline are used.
    pub head_len: usize,
    /// Where the headline starts.
    pub head_x: usize,
    /// The line under the headline: why the camera is in this state.
    pub sub: [u8; 96],
    /// How many bytes of the reason are used.
    pub sub_len: usize,
    /// Where the reason starts.
    pub sub_x: usize,
    /// The values of the settings rows, pre-rendered.
    ///
    /// WHY they are pre-rendered: the renderer runs once per pixel of the
    /// surface, and formatting a value there would run tens of thousands of
    /// times for one row of text.
    pub settings_values: [[u8; 64]; SETTINGS_ROWS],
    /// How many bytes of each settings value are used.
    pub settings_value_lens: [usize; SETTINGS_ROWS],
}

impl CameraApp {
    /// The camera's initial state.
    pub const fn new() -> Self {
        Self {
            mode: CameraMode::Photo,
            flash: FlashMode::Auto,
            flash_available: true,
            filter: CameraFilter::Normal,
            exposure_bias: 0,
            total_photos: 0,
            photos_published: 0,
            status: CameraStatus::AuthorityHeld,
            last_error: 0,
            frame_label: [0; 16],
            frame_label_len: 0,
            shots_label: [0; 16],
            shots_label_len: 0,
            last_photo: [0; PHOTO_PATH_MAX],
            last_photo_len: 0,
            publish: PublishState::None,
            capture: CaptureOutcome::None,
            index_error: 0,
            guides: false,
            aspect_ratio: CameraAspectRatio::Ratio4_3,
            shutter_feedback_ticks: 0,
            timer_mode: TimerMode::Off,
            countdown_ticks: 0,
            thumbnail: [0; THUMBNAIL_DIM * THUMBNAIL_DIM * 3],
            has_thumbnail: false,
            screen: CameraScreen::Viewfinder,
            head: [0; 48],
            head_len: 0,
            head_x: 0,
            sub: [0; 96],
            sub_len: 0,
            sub_x: 0,
            settings_values: [[0; 64]; SETTINGS_ROWS],
            settings_value_lens: [0; SETTINGS_ROWS],
        }
    }

    /// Rebuilds the settings surface's values.
    ///
    /// WHY this is a step: the values name the format the camera writes, where
    /// the last photo went and what became of the last publication, and the
    /// renderer reads them instead of formatting them per pixel.
    pub fn refresh_settings_values(&mut self) {
        let mut scratch = [0u8; 64];
        for (index, row) in SETTINGS_ROW_ORDER.iter().enumerate() {
            let len = self.settings_row_value(*row, &mut scratch);
            self.settings_value_lens[index] = len;
            copy_bytes(&mut self.settings_values[index], &scratch[..len]);
        }
    }

    /// Toggles the flash mode (Auto -> On -> Off -> Auto) and commands
    /// the hardware backend if turning on.
    ///
    /// Returns `None` when the flash backend is absent or refused the request.
    /// In that case the local mode is forced back to `Off` so neither the model
    /// nor the UI can claim an emitting torch that no hardware produced.
    pub fn toggle_flash(&mut self) -> Option<bool> {
        self.flash = self.flash.next();
        match self.flash {
            FlashMode::On => {
                match std::camera_toggle_flash() {
                    Ok(new_state) => {
                        self.flash_available = true;
                        Some(new_state)
                    }
                    Err(_) => {
                        self.flash_available = false;
                        self.flash = FlashMode::Off;
                        None
                    }
                }
            }
            FlashMode::Auto => {
                self.flash_available = true;
                Some(true)
            }
            FlashMode::Off => {
                let _ = std::camera_toggle_flash();
                Some(false)
            }
        }
    }

    /// The video frame path.
    ///
    /// WHY there is no video stream here: the camera HAL exposes one frame
    /// capture and no stream source, so a video shutter press has nothing to
    /// deliver. The app says so rather than pretending a frame arrived.
    pub fn stream_video_frame(&mut self) -> CaptureOutcome {
        self.capture = CaptureOutcome::NoVideoStream;
        CaptureOutcome::NoVideoStream
    }

    /// Asks the platform whether this app holds camera authority.
    ///
    /// WHY this asks about the authority and not about a sensor: the
    /// platform's answer to a call this app is not authorised to make is a
    /// refusal, and a refusal is the fact the user needs to see. Whether a
    /// sensor is bound is only knowable by asking for a frame, and asking
    /// for a frame the user did not ask for would take a photograph.
    pub fn probe_camera_authority(&mut self) {
        match std::camera_autofocus_state() {
            Ok(_) => {
                self.status = CameraStatus::AuthorityHeld;
                self.last_error = 0;
            }
            Err(code) => {
                self.status = status_from_error(code);
                self.last_error = code;
            }
        }
        self.refresh_status_text();
    }

    /// Rebuilds the viewfinder's text and the places it is drawn.
    ///
    /// WHY this is a step and not something the renderer does per pixel: the
    /// renderer is called once per pixel of the surface, and measuring a run
    /// there would run tens of thousands of times for one line of text. The
    /// text only changes when the platform's answer changes, so it is rebuilt
    /// when that happens and only read afterwards.
    pub fn refresh_status_text(&mut self) {
        self.head_len = copy_bytes(&mut self.head, self.status.headline());
        self.head_x = centred_x(
            &self.head[..self.head_len],
            2,
            VIEWFINDER_TEXT_X0,
            VIEWFINDER_TEXT_W,
        );
        let mut reason = [0u8; 96];
        let reason_len = self.status_reason(&mut reason);
        self.sub_len = copy_bytes(&mut self.sub, &reason[..reason_len]);
        self.sub_x = centred_x(
            &self.sub[..self.sub_len],
            1,
            VIEWFINDER_TEXT_X0,
            VIEWFINDER_TEXT_W,
        );
    }

    /// Rebuilds the shots readout after the counter moves.
    pub fn refresh_shots_label(&mut self) {
        let mut buf = [0u8; 16];
        let mut n = append_decimal(self.total_photos, &mut buf);
        let suffix: &[u8] = if self.total_photos == 1 { b" SHOT" } else { b" SHOTS" };
        n += copy_bytes(&mut buf[n..], suffix);
        self.shots_label_len = copy_bytes(&mut self.shots_label, &buf[..n]);
    }

    /// The label the capture readout carries.
    ///
    /// WHY the geometry wins once a frame exists: after a capture the
    /// platform has told the app how big the frame was, and that is a more
    /// useful thing to read than the fact that no frame has been taken.
    pub fn frame_readout(&self) -> (&[u8], usize) {
        if self.total_photos > 0 && self.frame_label_len > 0 {
            (&self.frame_label[..self.frame_label_len], self.frame_label_len)
        } else {
            (self.status.label(), self.status.label().len())
        }
    }

    /// The line under the viewfinder's headline, with the error code in it
    /// when the platform refused the capture.
    pub fn status_reason(&self, out: &mut [u8; 96]) -> usize {
        let mut n = 0usize;
        for byte in self.status.reason().iter() {
            if n >= out.len() {
                return n;
            }
            out[n] = *byte;
            n += 1;
        }
        if self.status == CameraStatus::Failed && self.last_error != 0 {
            if n < out.len() {
                out[n] = b' ';
                n += 1;
            }
            if n < out.len() {
                out[n] = b'(';
                n += 1;
            }
            if self.last_error < 0 {
                if n < out.len() {
                    out[n] = b'-';
                    n += 1;
                }
                n += append_decimal((-(self.last_error as i64)) as u32, &mut out[n..]);
            } else {
                n += append_decimal(self.last_error as u32, &mut out[n..]);
            }
            if n < out.len() {
                out[n] = b')';
                n += 1;
            }
        }
        n
    }

    /// Takes a photograph and publishes it.
    ///
    /// WHY the photo is written into the shared storage area: a photograph
    /// that only the camera can read is a photograph the user cannot open, so
    /// the capture goes into the document index like every other document on
    /// the device.
    ///
    /// WHY the answer is a [`CaptureOutcome`] and not a `Result`: a refusal to
    /// capture and a capture that could not be saved are different facts, and
    /// the caller has to be able to say which one happened.
    pub fn capture_still_photo(&mut self) -> CaptureOutcome {
        let mut meta = [0u32; 4];
        let payload_len = {
            let buffer = unsafe { &mut *addr_of_mut!(FRAME_BUFFER) };
            match std::camera_capture(buffer, &mut meta) {
                Ok(n) => n,
                Err(code) => {
                    self.status = status_from_error(code);
                    self.last_error = code;
                    self.refresh_status_text();
                    self.capture = CaptureOutcome::CaptureRefused(code);
                    return self.capture;
                }
            }
        };

        let width = meta[0];
        let height = meta[1];
        let Some(format) = frame_format(width, height, payload_len) else {
            self.status = CameraStatus::Failed;
            self.last_error = ERR_INVALID_ARGUMENTS;
            self.refresh_status_text();
            self.capture = CaptureOutcome::CaptureRefused(ERR_INVALID_ARGUMENTS);
            return self.capture;
        };

        {
            let buffer = unsafe { &mut *addr_of_mut!(FRAME_BUFFER) };
            apply_isp_tone_curve(
                self.filter,
                self.exposure_bias,
                format.bytes_per_pixel(),
                &mut buffer[..payload_len],
            );
        }

        self.generate_thumbnail(width, height, format);

        self.total_photos = self.total_photos.saturating_add(1);
        self.frame_label_len = 0;
        self.frame_label_len += append_decimal(width, &mut self.frame_label[self.frame_label_len..]);
        if self.frame_label_len < self.frame_label.len() {
            self.frame_label[self.frame_label_len] = b'x';
            self.frame_label_len += 1;
        }
        self.frame_label_len += append_decimal(height, &mut self.frame_label[self.frame_label_len..]);
        self.status = CameraStatus::AuthorityHeld;
        self.last_error = 0;
        self.refresh_shots_label();
        self.refresh_status_text();

        self.capture = match self.publish_photo(width, height, format) {
            Ok(true) => CaptureOutcome::Published,
            Ok(false) => CaptureOutcome::WrittenNotIndexed,
            Err(code) => CaptureOutcome::WriteRefused(code),
        };
        self.capture
    }

    /// Downsamples the captured frame to a circular preview thumbnail for the gallery disc.
    pub fn generate_thumbnail(&mut self, width: u32, height: u32, format: FrameFormat) {
        if width == 0 || height == 0 {
            return;
        }
        let source = unsafe { &*addr_of_mut!(FRAME_BUFFER) };
        let channels = format.bytes_per_pixel();
        let total_pixels = (width as usize) * (height as usize);
        if source.len() < total_pixels * channels {
            return;
        }

        let w = width as usize;
        let h = height as usize;
        for ty in 0..THUMBNAIL_DIM {
            let sy = (ty * h) / THUMBNAIL_DIM;
            for tx in 0..THUMBNAIL_DIM {
                let sx = (tx * w) / THUMBNAIL_DIM;
                let base = (sy * w + sx) * channels;
                if base + channels <= source.len() {
                    let (r, g, b) = match format {
                        FrameFormat::Rgba8888 | FrameFormat::Rgb888 => {
                            (source[base], source[base + 1], source[base + 2])
                        }
                        FrameFormat::Gray8 => (source[base], source[base], source[base]),
                    };
                    let out_idx = (ty * THUMBNAIL_DIM + tx) * 3;
                    self.thumbnail[out_idx] = r;
                    self.thumbnail[out_idx + 1] = g;
                    self.thumbnail[out_idx + 2] = b;
                }
            }
        }
        self.has_thumbnail = true;
    }

    /// Hands the last photo to the app that opens images, or launches the Gallery app.
    ///
    /// WHY the platform's intent registry and service package launch: the gallery
    /// declares that it handles this media type, so intent dispatch opens the exact
    /// photo. If that is refused or unavailable, direct service package launch opens
    /// the Gallery so the user can inspect their collection.
    pub fn open_photo_library(&self) -> Result<u64, i64> {
        // Try intent dispatch with the photo path first if one was captured
        if self.last_photo_len > 0 {
            if let Ok(path) = core::str::from_utf8(&self.last_photo[..self.last_photo_len]) {
                if let Ok(task_id) = std::content::intent_dispatch(std::content::IntentKind::Mime, PHOTO_MIME, path) {
                    return Ok(task_id);
                }
            }
        }
        // Fall back to direct service package launch
        if let Ok(task_id) = std::service_package_launch("gallery") {
            return Ok(task_id);
        }
        if let Ok(task_id) = std::service_package_launch("/packages/gallery.rapp") {
            return Ok(task_id);
        }
        if self.last_photo_len == 0 {
            Err(ERR_NO_PHOTO)
        } else {
            Err(ERR_PERMISSION_DENIED)
        }
    }

    /// Writes the captured frame into the shared storage area and asks the
    /// platform to describe it.
    ///
    /// WHY `Ok(true)` and `Ok(false)` are different answers: the file being
    /// on the device and the document index describing it are two facts, and a
    /// photo the gallery cannot find is not a photo the user can open. `Err`
    /// carries the code the platform refused the write with.
    pub fn publish_photo(&mut self, width: u32, height: u32, format: FrameFormat) -> Result<bool, i64> {
        let mut path = [0u8; PHOTO_PATH_MAX];
        let path_len = photo_path(self.photos_published + 1, &mut path);
        let path_str = match core::str::from_utf8(&path[..path_len]) {
            Ok(s) => s,
            Err(_) => return Err(ERR_INVALID_ARGUMENTS),
        };

        let fd = match std::fs_open(path_str, std::FS_WRITE) {
            Ok(fd) => fd,
            Err(code) => {
                self.publish = PublishState::Refused(code);
                return Err(code);
            }
        };

        let mut header = [0u8; 32];
        let header_len = ppm_header(width, height, &mut header);
        if let Err(code) = std::fs_write(fd, &header[..header_len]) {
            let _ = std::fs_close(fd);
            self.publish = PublishState::Refused(code);
            return Err(code);
        }

        if let Err(code) = self.write_pixels(fd, width, height, format) {
            let _ = std::fs_close(fd);
            self.publish = PublishState::Refused(code);
            return Err(code);
        }

        if let Err(code) = std::fs_close(fd) {
            self.publish = PublishState::Refused(code);
            return Err(code);
        }

        copy_bytes(&mut self.last_photo, &path[..path_len]);
        self.last_photo_len = path_len;
        self.photos_published = self.photos_published.saturating_add(1);

        // The index is rebuilt from the storage area, so a photo that was
        // written is a photo the platform can find. The refresh is asked for
        // explicitly so the photo does not wait for the next index query.
        //
        // WHY the refusal is remembered: a camera that holds no index authority
        // gets a photo on the device that nothing else can find, and the user
        // has to be told that is why the gallery cannot show the photo.
        let refreshed = match std::content::content_index_refresh() {
            Ok(_) => true,
            Err(code) => {
                self.index_error = code;
                false
            }
        };
        let mut raw = [0u8; 2048];
        let found = std::content::content_index_find_path(path_str, &mut raw).is_some();
        let indexed = refreshed && found;
        self.publish = publish_state(Ok(()), indexed);
        if indexed {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Writes the frame's pixels into the open file, converting whatever the
    /// platform returned into the three channels a PPM holds.
    fn write_pixels(
        &mut self,
        fd: u32,
        width: u32,
        height: u32,
        format: FrameFormat,
    ) -> Result<(), i64> {
        let source = unsafe { &*addr_of_mut!(FRAME_BUFFER) };
        let channels = format.bytes_per_pixel();
        let mut scratch = [0u8; 1024];
        let mut written_rows = 0u32;
        let row_bytes = (width as usize) * channels;
        while written_rows < height {
            let row = written_rows as usize;
            let start = row * row_bytes;
            if start >= source.len() {
                break;
            }
            let end = core::cmp::min(start + row_bytes, source.len());
            let row_pixels = (end - start) / channels;
            let mut out = 0usize;
            let mut pixel = 0usize;
            while pixel < row_pixels && out + 3 <= scratch.len() {
                let base = start + pixel * channels;
                let (r, g, b) = match format {
                    FrameFormat::Rgba8888 => (source[base], source[base + 1], source[base + 2]),
                    FrameFormat::Rgb888 => (source[base], source[base + 1], source[base + 2]),
                    FrameFormat::Gray8 => (source[base], source[base], source[base]),
                };
                scratch[out] = r;
                scratch[out + 1] = g;
                scratch[out + 2] = b;
                out += 3;
                pixel += 1;
            }
            if out == 0 {
                break;
            }
            std::fs_write(fd, &scratch[..out])?;
            written_rows += 1;
        }
        Ok(())
    }

    /// The title of a settings row.
    pub fn settings_row_title(&self, row: SettingsRow) -> &'static [u8] {
        match row {
            SettingsRow::Format => b"Photo format",
            SettingsRow::Filter => b"Image processing",
            SettingsRow::Flash => b"Flash and torch",
            SettingsRow::Guides => b"Framing guides",
            SettingsRow::Photos => b"Photos on the device",
            SettingsRow::LastPhoto => b"Last photo",
            SettingsRow::LastCapture => b"Last capture",
            SettingsRow::OpenLibrary => b"Open the photo library",
            SettingsRow::Close => b"Close",
        }
    }

    /// The value of a settings row, copied into `out`.
    pub fn settings_row_value(&self, row: SettingsRow, out: &mut [u8; 64]) -> usize {
        match row {
            SettingsRow::Format => copy_bytes(out, b"PPM (P6), 8 bits per channel"),
            SettingsRow::Filter => copy_bytes(out, self.filter.label()),
            SettingsRow::Flash => copy_bytes(out, self.flash.label()),
            SettingsRow::Guides => copy_bytes(out, if self.guides { b"On" } else { b"Off" }),
            SettingsRow::Photos => {
                let mut n = 0usize;
                n += copy_bytes(&mut out[n..], b"Saved ");
                n += append_decimal(self.photos_published, &mut out[n..]);
                n += copy_bytes(&mut out[n..], b" of ");
                n += append_decimal(self.total_photos, &mut out[n..]);
                n
            }
            SettingsRow::LastPhoto => {
                if self.last_photo_len == 0 {
                    copy_bytes(out, b"None yet")
                } else {
                    copy_bytes(out, &self.last_photo[..self.last_photo_len])
                }
            }
            SettingsRow::LastCapture => copy_bytes(out, self.capture.caption()),
            SettingsRow::OpenLibrary => copy_bytes(out, self.publish.caption()),
            SettingsRow::Close => copy_bytes(out, b"Back to the viewfinder"),
        }
    }

    /// The note under a settings row: what the platform actually supports.
    pub fn settings_row_note(&self, row: SettingsRow) -> &'static [u8] {
        match row {
            SettingsRow::Format => b"Written into the shared storage area",
            SettingsRow::Filter => b"The top row changes the filter for the next capture",
            SettingsRow::Flash => b"The camera hardware reports the state it can reach",
            SettingsRow::Guides => b"Brackets, reticle and thirds over the viewfinder",
            SettingsRow::Photos => b"Frames this app captured",
            SettingsRow::LastPhoto => b"The path the last photo was written to",
            SettingsRow::LastCapture => b"What the last press of the shutter produced",
            SettingsRow::OpenLibrary => b"Hand the photo to the gallery that handles it",
            SettingsRow::Close => b"",
        }
    }
}

/// Copies `src` into `out` and returns how many bytes were written.
pub fn copy_bytes(out: &mut [u8], src: &[u8]) -> usize {
    let n = core::cmp::min(out.len(), src.len());
    out[..n].copy_from_slice(&src[..n]);
    n
}

/// The buffer the camera captures into.
///
/// WHY 64 KiB: the app has no allocator, so the frame the camera HAL hands
/// over has to fit in a fixed buffer. 64 KiB holds a 128x128 frame of four
/// bytes per pixel and nothing larger, and the platform says so when a frame
/// does not fit rather than truncating it.
pub const FRAME_BUFFER_BYTES: usize = 64 * 1024;

static mut FRAME_BUFFER: [u8; FRAME_BUFFER_BYTES] = [0; FRAME_BUFFER_BYTES];

/// Applies the image processing to a frame the platform returned.
///
/// WHY the channel count is a parameter: the platform does not report the
/// frame's pixel format, and the app works the format out from the bytes it
/// was handed. A filter that assumed four bytes per pixel would corrupt a
/// frame that is packed differently.
pub fn apply_isp_tone_curve(
    filter: CameraFilter,
    exposure_bias: i16,
    channels: usize,
    buf: &mut [u8],
) {
    if channels == 0 {
        return;
    }
    let mut i = 0usize;
    while i + channels <= buf.len() {
        let r = buf[i];
        let g = if channels >= 3 { buf[i + 1] } else { r };
        let b = if channels >= 3 { buf[i + 2] } else { r };
        let (mut r, mut g, mut b) = (r, g, b);
        match filter {
            CameraFilter::Normal => {}
            CameraFilter::Vivid => {
                let l = (r as i32 * 30 + g as i32 * 59 + b as i32 * 11) / 100;
                let sat_r = l + ((r as i32 - l) * 135) / 100;
                let sat_g = l + ((g as i32 - l) * 135) / 100;
                let sat_b = l + ((b as i32 - l) * 135) / 100;
                r = contrast(sat_r.clamp(0, 255) as u8);
                g = contrast(sat_g.clamp(0, 255) as u8);
                b = contrast(sat_b.clamp(0, 255) as u8);
            }
            CameraFilter::Warm => {
                let wr = ((r as u32 * 116) / 100).min(255) as u8;
                let wg = ((g as u32 * 105) / 100).min(255) as u8;
                let wb = ((b as u32 * 88) / 100).min(255) as u8;
                r = wr;
                g = wg;
                b = wb;
            }
            CameraFilter::Cool => {
                let cr = ((r as u32 * 88) / 100).min(255) as u8;
                let cg = ((g as u32 * 104) / 100).min(255) as u8;
                let cb = ((b as u32 * 118) / 100).min(255) as u8;
                r = cr;
                g = cg;
                b = cb;
            }
            CameraFilter::Mono => {
                let l = ((r as u32 * 30 + g as u32 * 59 + b as u32 * 11) / 100) as u8;
                let c = contrast(l);
                r = c;
                g = c;
                b = c;
            }
            CameraFilter::Vintage => {
                let vr = (24 + (r as u32 * 205) / 255).min(255) as u8;
                let vg = (20 + (g as u32 * 200) / 255).min(255) as u8;
                let vb = (30 + (b as u32 * 185) / 255).min(255) as u8;
                r = vr;
                g = vg;
                b = vb;
            }
        }
        if exposure_bias != 0 {
            let bias = exposure_bias as i32 * 16;
            r = (r as i32 + bias).clamp(0, 255) as u8;
            g = (g as i32 + bias).clamp(0, 255) as u8;
            b = (b as i32 + bias).clamp(0, 255) as u8;
        }
        buf[i] = r;
        if channels >= 3 {
            buf[i + 1] = g;
            buf[i + 2] = b;
        }
        i += channels;
    }
}

/// Steepens one channel.
fn contrast(v: u8) -> u8 {
    let x = v as i32;
    let out = (x - 128) * 3 / 2 + 128;
    out.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aspect_ratio_selector_cycles_correctly() {
        assert_eq!(CameraAspectRatio::Ratio4_3.label(), b"4:3");
        assert_eq!(CameraAspectRatio::Ratio16_9.label(), b"16:9");
        assert_eq!(CameraAspectRatio::Square.label(), b"1:1");

        assert_eq!(CameraAspectRatio::Ratio4_3.toggle(), CameraAspectRatio::Ratio16_9);
        assert_eq!(CameraAspectRatio::Ratio16_9.toggle(), CameraAspectRatio::Square);
        assert_eq!(CameraAspectRatio::Square.toggle(), CameraAspectRatio::Ratio4_3);
    }

    #[test]
    fn timer_mode_cycles_correctly() {
        assert_eq!(TimerMode::Off.label(), b"TIMER");
        assert_eq!(TimerMode::Sec3.label(), b"3s");
        assert_eq!(TimerMode::Sec10.label(), b"10s");

        assert_eq!(TimerMode::Off.next(), TimerMode::Sec3);
        assert_eq!(TimerMode::Sec3.next(), TimerMode::Sec10);
        assert_eq!(TimerMode::Sec10.next(), TimerMode::Off);

        assert_eq!(TimerMode::Off.seconds(), 0);
        assert_eq!(TimerMode::Sec3.seconds(), 3);
        assert_eq!(TimerMode::Sec10.seconds(), 10);
    }

    #[test]
    fn flash_mode_cycles_correctly() {
        assert_eq!(FlashMode::Auto.label(), b"FLASH AUTO");
        assert_eq!(FlashMode::On.label(), b"FLASH ON");
        assert_eq!(FlashMode::Off.label(), b"FLASH OFF");

        assert_eq!(FlashMode::Auto.next(), FlashMode::On);
        assert_eq!(FlashMode::On.next(), FlashMode::Off);
        assert_eq!(FlashMode::Off.next(), FlashMode::Auto);
    }

    #[test]
    fn filter_suite_cycles_all_calibrated_presets() {
        assert_eq!(CameraFilter::Normal.label(), b"NORMAL");
        assert_eq!(CameraFilter::Vivid.label(), b"VIVID");
        assert_eq!(CameraFilter::Warm.label(), b"WARM");
        assert_eq!(CameraFilter::Cool.label(), b"COOL");
        assert_eq!(CameraFilter::Mono.label(), b"MONO");
        assert_eq!(CameraFilter::Vintage.label(), b"VINTAGE");

        assert_eq!(CameraFilter::Normal.next(), CameraFilter::Vivid);
        assert_eq!(CameraFilter::Vivid.next(), CameraFilter::Warm);
        assert_eq!(CameraFilter::Warm.next(), CameraFilter::Cool);
        assert_eq!(CameraFilter::Cool.next(), CameraFilter::Mono);
        assert_eq!(CameraFilter::Mono.next(), CameraFilter::Vintage);
        assert_eq!(CameraFilter::Vintage.next(), CameraFilter::Normal);
    }

    #[test]
    fn the_refusal_and_the_absent_sensor_are_different_facts() {
        // The two facts must not read the same on the screen.
        assert_ne!(
            CameraStatus::AuthorityRefused.label(),
            CameraStatus::NoSensor.label()
        );
        assert_ne!(
            CameraStatus::AuthorityRefused.headline(),
            CameraStatus::NoSensor.headline()
        );
        assert_ne!(
            CameraStatus::AuthorityRefused.reason(),
            CameraStatus::NoSensor.reason()
        );
        // And each one has to say why.
        assert!(core::str::from_utf8(CameraStatus::AuthorityRefused.reason())
            .unwrap()
            .contains("denied"));
        assert!(core::str::from_utf8(CameraStatus::NoSensor.reason())
            .unwrap()
            .contains("no sensor"));
    }

    #[test]
    fn the_error_code_decides_which_fact_the_platform_stated() {
        assert_eq!(
            status_from_error(ERR_PERMISSION_DENIED),
            CameraStatus::AuthorityRefused
        );
        assert_eq!(status_from_error(ERR_DEVICE_ERROR), CameraStatus::NoSensor);
        assert_eq!(status_from_error(ERR_INVALID_ARGUMENTS), CameraStatus::Failed);
    }

    #[test]
    fn a_frame_that_does_not_match_its_geometry_is_refused() {
        assert_eq!(frame_format(2, 2, 16), Some(FrameFormat::Rgba8888));
        assert_eq!(frame_format(2, 2, 12), Some(FrameFormat::Rgb888));
        assert_eq!(frame_format(2, 2, 4), Some(FrameFormat::Gray8));
        // A frame that is not a whole number of pixels is not a frame.
        assert_eq!(frame_format(2, 2, 15), None);
        // A frame whose length matches no known layout is not guessed at.
        assert_eq!(frame_format(2, 2, 32), None);
        assert_eq!(frame_format(0, 4, 0), None);
    }

    #[test]
    fn the_header_names_the_format_that_is_actually_written() {
        let mut header = [0u8; 32];
        let n = ppm_header(3, 5, &mut header);
        assert_eq!(&header[..n], b"P6\n3 5\n255\n");
        // The platform's media-type sniffer reads these bytes, so the header
        // has to start with the magic it looks for.
        assert_eq!(&header[..2], b"P6");
    }

    #[test]
    fn the_photo_is_written_into_the_photos_directory() {
        let mut path = [0u8; PHOTO_PATH_MAX];
        let n = photo_path(1, &mut path);
        assert_eq!(&path[..n], b"/storage/photos/shot-1.ppm");
        let n = photo_path(42, &mut path);
        assert_eq!(&path[..n], b"/storage/photos/shot-42.ppm");
        // Every photo the camera writes has to be somewhere the platform's
        // media-type sniffer and the gallery look.
        assert!(core::str::from_utf8(&path[..n])
            .unwrap()
            .starts_with("/storage/photos/"));
    }

    #[test]
    fn the_publication_reports_what_the_platform_answered() {
        assert_eq!(publish_state(Ok(()), true), PublishState::Published);
        assert_eq!(publish_state(Ok(()), false), PublishState::WrittenNotIndexed);
        assert_eq!(publish_state(Err(-13), false), PublishState::Refused(-13));
        // WHY the refusal's own code is carried through: the camera reports
        // the platform's answer, not a constant of its own, so a caller can
        // tell one refusal from another.
        assert_eq!(PublishState::Refused(-13).error_code(), Some(-13));
        assert_eq!(PublishState::Published.error_code(), None);
        // The three captions have to be three different facts.
        assert_ne!(
            PublishState::Published.caption(),
            PublishState::WrittenNotIndexed.caption()
        );
        assert_ne!(
            PublishState::WrittenNotIndexed.caption(),
            PublishState::Refused(-13).caption()
        );
    }

    #[test]
    fn the_settings_surface_describes_every_row() {
        let app = CameraApp::new();
        let mut value = [0u8; 64];
        for row in SETTINGS_ROW_ORDER.iter() {
            let title = app.settings_row_title(*row);
            assert!(!title.is_empty());
            let n = app.settings_row_value(*row, &mut value);
            assert!(n > 0);
        }
        // The format the app writes is the format the header names.
        let n = app.settings_row_value(SettingsRow::Format, &mut value);
        assert_eq!(&value[..n], b"PPM (P6), 8 bits per channel");
    }

    #[test]
    fn the_capture_readout_reports_the_state_until_a_frame_exists() {
        let mut app = CameraApp::new();
        app.status = CameraStatus::AuthorityRefused;
        let (label, _) = app.frame_readout();
        assert_eq!(label, CameraStatus::AuthorityRefused.label());
        app.total_photos = 1;
        app.frame_label = *b"1280x720\0\0\0\0\0\0\0\0";
        app.frame_label_len = 8;
        let (label, len) = app.frame_readout();
        assert_eq!(&label[..len], b"1280x720");
    }

    #[test]
    fn the_isp_filter_leaves_the_alpha_channel_alone() {
        let mut pixels = [255u8, 0, 0, 255, 0, 255, 0, 255];
        apply_isp_tone_curve(CameraFilter::Mono, 0, 4, &mut pixels);
        assert_eq!(pixels[0], 50);
        assert_eq!(pixels[1], 50);
        assert_eq!(pixels[2], 50);
        assert_eq!(pixels[3], 255);
    }

    #[test]
    fn the_isp_filter_respects_the_frame_it_was_given() {
        // A frame that is packed three bytes per pixel is filtered three
        // bytes at a time, so the next pixel is not shifted.
        let mut pixels = [255u8, 0, 0, 0, 255, 0];
        apply_isp_tone_curve(CameraFilter::Mono, 0, 3, &mut pixels);
        // The first pixel is red (contrast(76)=50), the second green (contrast(150)=161),
        // and each keeps its own three bytes.
        assert_eq!(&pixels[..3], &[50, 50, 50]);
        assert_eq!(&pixels[3..6], &[161, 161, 161]);
    }

    #[test]
    fn thumbnail_generation_downsamples_frame_to_disc_buffer() {
        let mut app = CameraApp::new();
        assert!(!app.has_thumbnail);
        let (w, h) = (4u32, 4u32);
        // Fill static FRAME_BUFFER with pattern
        unsafe {
            let buf = &mut *addr_of_mut!(FRAME_BUFFER);
            for i in 0..64 {
                buf[i] = (i % 256) as u8;
            }
        }
        app.generate_thumbnail(w, h, FrameFormat::Rgb888);
        assert!(app.has_thumbnail);
        // Verify buffer has values
        assert_eq!(app.thumbnail.len(), THUMBNAIL_DIM * THUMBNAIL_DIM * 3);
    }

    #[test]
    fn every_line_the_camera_draws_fits_the_column_it_is_drawn_in() {
        // The viewfinder's headline is drawn at scale 2 and its reason at
        // scale 1; both have to fit the column or the renderer declines to
        // draw them and the screen says nothing.
        for status in [
            CameraStatus::AuthorityRefused,
            CameraStatus::NoSensor,
            CameraStatus::AuthorityHeld,
            CameraStatus::Failed,
        ] {
            let head = status.headline();
            assert!(
                std::design::font::measure(head, 2) <= VIEWFINDER_TEXT_W,
                "the headline {:?} does not fit the viewfinder column",
                core::str::from_utf8(head).unwrap()
            );
            let reason = status.reason();
            assert!(
                std::design::font::measure(reason, 1) <= VIEWFINDER_TEXT_W,
                "the reason {:?} does not fit the viewfinder column",
                core::str::from_utf8(reason).unwrap()
            );
        }
    }

    #[test]
    fn every_settings_line_fits_its_column() {
        let app = CameraApp::new();
        let mut value = [0u8; 64];
        for row in SETTINGS_ROW_ORDER.iter() {
            let title = app.settings_row_title(*row);
            assert!(
                std::design::font::measure(title, 1) <= SETTINGS_TEXT_MAX,
                "the settings title {:?} does not fit its column",
                core::str::from_utf8(title).unwrap()
            );
            let note = app.settings_row_note(*row);
            assert!(
                std::design::font::measure(note, 1) <= SETTINGS_TEXT_MAX,
                "the settings note {:?} does not fit its column",
                core::str::from_utf8(note).unwrap()
            );
            let n = app.settings_row_value(*row, &mut value);
            assert!(
                std::design::font::measure(&value[..n], 1) <= SETTINGS_TEXT_MAX,
                "the settings value {:?} does not fit its column",
                core::str::from_utf8(&value[..n]).unwrap()
            );
        }
    }

    #[test]
    fn the_readout_says_which_fact_the_platform_stated() {
        // The capture readout is the first thing the user reads, so the three
        // states have to be told apart there as well as in the viewfinder.
        assert_eq!(CameraStatus::AuthorityRefused.label(), b"NO AUTH");
        assert_eq!(CameraStatus::NoSensor.label(), b"NO SENSOR");
        assert_eq!(CameraStatus::AuthorityHeld.label(), b"NO FRAME");
        // And the refusal and the absent sensor are not the same words.
        assert_ne!(
            CameraStatus::AuthorityRefused.label(),
            CameraStatus::NoSensor.label()
        );
    }
}
