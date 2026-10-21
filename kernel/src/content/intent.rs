//! The system-wide intent registry.
//!
//! WHY this lives in the kernel and not in an app: "who handles this file?"
//! is a question about the whole device, and an answer that lives inside one
//! app is an answer that app can edit. The registry here is built from the
//! packages' own signed manifests, so the set of handlers is a property of
//! what is installed rather than of what any single program believes.
//!
//! WHY a refusal is a refusal: when nothing declares a handler for a media
//! type, a scheme or an action, the registry says so and names nothing. A
//! registry that guessed would be the same private table this replaces, only
//! further from the truth.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a handler declares it can handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentKind {
    /// A media type, e.g. `image/png`.
    Mime,
    /// A URI scheme, e.g. `https`.
    Scheme,
    /// A named action, e.g. `org.interos.action.set_wallpaper`.
    Action,
}

impl IntentKind {
    /// The wire value of the kind, as it appears in a syscall argument and in
    /// the RAPP intent table.
    pub const fn as_raw(self) -> u64 {
        match self {
            IntentKind::Mime => 0,
            IntentKind::Scheme => 1,
            IntentKind::Action => 2,
        }
    }

    /// The wire value of the kind, as it appears in a listing.
    pub const fn as_str(self) -> &'static str {
        match self {
            IntentKind::Mime => "mime",
            IntentKind::Scheme => "scheme",
            IntentKind::Action => "action",
        }
    }

    /// Resolves a wire value back to a kind.
    pub const fn from_raw(value: u64) -> Option<Self> {
        match value {
            0 => Some(IntentKind::Mime),
            1 => Some(IntentKind::Scheme),
            2 => Some(IntentKind::Action),
            _ => None,
        }
    }
}

/// One thing a package declares it can handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentDeclaration {
    /// Which sort of thing this is.
    pub kind: IntentKind,
    /// The value, e.g. `image/png`.
    pub value: String,
}

impl IntentDeclaration {
    /// Builds a declaration.
    pub fn new(kind: IntentKind, value: &str) -> Self {
        Self {
            kind,
            value: value.to_string(),
        }
    }
}

/// One registered handler: a package's declaration of what it can handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentHandler {
    /// Which sort of thing this handler takes.
    pub kind: IntentKind,
    /// The value the handler takes.
    pub value: String,
    /// The package that declared it.
    pub package_id: String,
}

/// An intent that was dispatched and is waiting for its handler to collect it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingIntent {
    /// The package the intent was dispatched to.
    pub package_id: String,
    /// The kind of the intent.
    pub kind: IntentKind,
    /// The value of the intent.
    pub value: String,
    /// The payload, normally a document path.
    pub payload: String,
    /// A counter that lets the collector tell a fresh intent from a stale one.
    pub token: u64,
}

/// The registry of every handler the installed packages have declared.
#[derive(Debug, Clone)]
pub struct IntentRegistry {
    handlers: Vec<IntentHandler>,
    pending: Option<PendingIntent>,
    next_token: u64,
}

impl IntentRegistry {
    /// An empty registry.
    pub const fn new() -> Self {
        Self {
            handlers: Vec::new(),
            pending: None,
            next_token: 1,
        }
    }

    /// Replaces a package's declarations with the ones it now declares.
    ///
    /// WHY replacement and not accumulation: a package that is upgraded or
    /// uninstalled must not leave a handler behind that no longer belongs to
    /// anything, and a package that declares the same media type twice must
    /// not appear twice in the "open with" list.
    pub fn register(&mut self, package_id: &str, declarations: &[IntentDeclaration]) {
        self.unregister(package_id);
        for declaration in declarations {
            if declaration.value.is_empty() {
                continue;
            }
            self.handlers.push(IntentHandler {
                kind: declaration.kind,
                value: declaration.value.clone(),
                package_id: package_id.to_string(),
            });
        }
    }

    /// Removes every declaration belonging to a package.
    pub fn unregister(&mut self, package_id: &str) {
        self.handlers
            .retain(|handler| handler.package_id != package_id);
        if let Some(pending) = &self.pending {
            if pending.package_id == package_id {
                self.pending = None;
            }
        }
    }

    /// The package that handles a kind and value, if any package declares it.
    ///
    /// WHY the first declaration wins: a media type may be declared by more
    /// than one package, and the platform needs one answer to dispatch. The
    /// full list is available through [`Self::handlers_for`], and the choice
    /// is the first package that claimed it, which is stable across boots
    /// because the registry is built in package-id order.
    pub fn resolve(&self, kind: IntentKind, value: &str) -> Option<&str> {
        self.handlers
            .iter()
            .find(|handler| handler.kind == kind && handler.value == value)
            .map(|handler| handler.package_id.as_str())
    }

    /// Every package that declares a handler for a kind and value.
    pub fn handlers_for(&self, kind: IntentKind, value: &str) -> Vec<&str> {
        self.handlers
            .iter()
            .filter(|handler| handler.kind == kind && handler.value == value)
            .map(|handler| handler.package_id.as_str())
            .collect()
    }

    /// Every registered handler, in registration order.
    pub fn handlers(&self) -> &[IntentHandler] {
        &self.handlers
    }

    /// Records an intent for its handler to collect.
    pub fn set_pending(&mut self, kind: IntentKind, value: &str, package_id: &str, payload: &str) -> u64 {
        let token = self.next_token;
        self.next_token = self.next_token.wrapping_add(1).max(1);
        self.pending = Some(PendingIntent {
            package_id: package_id.to_string(),
            kind,
            value: value.to_string(),
            payload: payload.to_string(),
            token,
        });
        token
    }

    /// The intent waiting for a package, if it was the one dispatched to.
    ///
    /// WHY the package id is checked: the pending intent is a device-wide
    /// slot, and a program that could read another program's intent would be
    /// able to learn what the user is about to open.
    pub fn take_pending(&mut self, package_id: &str) -> Option<PendingIntent> {
        let pending = self.pending.as_ref()?;
        if pending.package_id != package_id {
            return None;
        }
        self.pending.take()
    }

    /// The intent waiting for a package, without consuming it.
    pub fn peek_pending(&self, package_id: &str) -> Option<&PendingIntent> {
        let pending = self.pending.as_ref()?;
        if pending.package_id == package_id {
            Some(pending)
        } else {
            None
        }
    }
}

/// The device's intent registry.
pub static INTENT_REGISTRY: spin::Mutex<IntentRegistry> = spin::Mutex::new(IntentRegistry::new());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_handler_is_the_only_thing_that_handles_its_value() {
        let mut registry = IntentRegistry::new();
        registry.register(
            "org.interos.gallery",
            &[IntentDeclaration::new(IntentKind::Mime, "image/png")],
        );
        assert_eq!(
            registry.resolve(IntentKind::Mime, "image/png"),
            Some("org.interos.gallery")
        );
        assert_eq!(registry.resolve(IntentKind::Mime, "image/jpeg"), None);
        assert_eq!(registry.resolve(IntentKind::Action, "image/png"), None);
    }

    #[test]
    fn an_unhandled_value_resolves_to_nothing() {
        let registry = IntentRegistry::new();
        assert_eq!(registry.resolve(IntentKind::Mime, "application/x-executable"), None);
        assert!(registry.handlers_for(IntentKind::Mime, "application/x-executable").is_empty());
    }

    #[test]
    fn two_packages_may_declare_the_same_media_type_and_both_are_listed() {
        let mut registry = IntentRegistry::new();
        registry.register(
            "org.interos.gallery",
            &[IntentDeclaration::new(IntentKind::Mime, "image/png")],
        );
        registry.register(
            "org.interos.editor",
            &[IntentDeclaration::new(IntentKind::Mime, "image/png")],
        );
        let handlers = registry.handlers_for(IntentKind::Mime, "image/png");
        assert_eq!(handlers.len(), 2);
        assert!(handlers.contains(&"org.interos.gallery"));
        assert!(handlers.contains(&"org.interos.editor"));
    }

    #[test]
    fn re_registering_a_package_replaces_its_declarations() {
        let mut registry = IntentRegistry::new();
        registry.register(
            "org.interos.gallery",
            &[
                IntentDeclaration::new(IntentKind::Mime, "image/png"),
                IntentDeclaration::new(IntentKind::Mime, "image/jpeg"),
            ],
        );
        registry.register(
            "org.interos.gallery",
            &[IntentDeclaration::new(IntentKind::Mime, "image/png")],
        );
        assert_eq!(registry.resolve(IntentKind::Mime, "image/png"), Some("org.interos.gallery"));
        assert_eq!(registry.resolve(IntentKind::Mime, "image/jpeg"), None);
    }

    #[test]
    fn uninstalling_a_package_removes_its_handlers() {
        let mut registry = IntentRegistry::new();
        registry.register(
            "org.interos.gallery",
            &[IntentDeclaration::new(IntentKind::Mime, "image/png")],
        );
        registry.unregister("org.interos.gallery");
        assert_eq!(registry.resolve(IntentKind::Mime, "image/png"), None);
    }

    #[test]
    fn a_pending_intent_is_only_visible_to_the_package_it_was_dispatched_to() {
        let mut registry = IntentRegistry::new();
        registry.set_pending(
            IntentKind::Action,
            "org.interos.action.set_wallpaper",
            "org.interos.settings",
            "/storage/photos/shot.ppm",
        );
        assert!(registry.peek_pending("org.interos.gallery").is_none());
        let taken = registry
            .take_pending("org.interos.settings")
            .expect("the dispatched package sees the intent");
        assert_eq!(taken.payload, "/storage/photos/shot.ppm");
        assert!(registry.peek_pending("org.interos.settings").is_none());
    }
}
