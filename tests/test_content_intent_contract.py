# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Contract: the content layer -- one shared document index, one intent registry.

WHY this contract exists: the device used to have no shared description of the
documents it held and no shared answer to "which program opens this file". The
gallery scanned the storage area with its own parser, the file manager kept a
private table of which application opened which media type, and the two could
disagree. This contract pins the rules that replaced that:

* an intent that no package handles is refused, and the refusal names nothing;
* the index says where a media type came from, so a guess is never presented
  as a measurement;
* a package's own declaration is the only thing that can handle its media
  type;
* the interlinks between the gallery, the file manager, the browser, the
  settings app, the dialer and the launcher are present and real.
"""

import ast
import importlib.util
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
KERNEL = ROOT / "kernel" / "src"
SDK = ROOT / "sdk" / "rust" / "interos-std" / "src"
APPS = ROOT / "apps"
TOOLS = ROOT / "tools"

CONTENT = KERNEL / "content"
INDEX_RS = CONTENT / "index.rs"
INTENT_RS = CONTENT / "intent.rs"
MIME_RS = CONTENT / "mime.rs"
SYSCALL_CONTENT_RS = KERNEL / "syscall" / "content.rs"
CAPABILITY_RS = KERNEL / "capability" / "mod.rs"
RAPP_RS = KERNEL / "package" / "rapp.rs"
PACKAGE_RS = KERNEL / "package" / "mod.rs"
VFS_RS = KERNEL / "fs" / "vfs.rs"
SDK_CONTENT_RS = SDK / "content.rs"


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


# -----------------------------------------------------------------------------
# The kernel holds the subsystem, and it is reachable only through a capability
# -----------------------------------------------------------------------------
def test_the_kernel_holds_a_content_subsystem():
    for module in ("mod.rs", "index.rs", "intent.rs", "mime.rs"):
        assert (CONTENT / module).is_file(), f"the content subsystem is missing {module}"
    lib = read(KERNEL / "lib.rs")
    assert "pub mod content;" in lib, "the kernel does not declare the content module"


def test_the_content_surface_needs_a_capability_and_not_a_slot():
    """WHY: a slot is a position in the task's table; the capability *type* is
    what the operation means, and the platform authorises by type."""
    syscall = read(SYSCALL_CONTENT_RS)
    assert "CapabilityType::ContentIndexRead" in syscall
    assert "CapabilityType::ContentIntentDispatch" in syscall
    # The authority is taken from the caller's own capability table.
    assert "authorize_capability" in syscall
    assert "cap_slot" not in syscall.lower()


def test_the_two_new_capabilities_are_in_the_abi_table():
    capability = read(CAPABILITY_RS)
    assert "ContentIndexRead" in capability
    assert "ContentIntentDispatch" in capability
    # The identifiers are fixed by the ABI and must not drift.
    assert re.search(r"CapabilityType::ContentIndexRead\s*=>\s*25", capability)
    assert re.search(r"CapabilityType::ContentIntentDispatch\s*=>\s*26", capability)
    # And the RAPP permission table has to know them.
    interos = read(TOOLS / "interos.py")
    assert '"CAP_CONTENT_INDEX": 25' in interos
    assert '"CAP_CONTENT_INTENT": 26' in interos


def test_the_index_and_the_registry_are_separate_calls():
    syscall = read(SYSCALL_CONTENT_RS)
    assert "fn sys_content_index_query" in syscall
    assert "fn sys_content_index_refresh" in syscall
    assert "fn sys_intent_resolve" in syscall
    assert "fn sys_intent_list" in syscall
    assert "fn sys_intent_dispatch" in syscall
    assert "fn sys_intent_pending" in syscall


def test_the_query_surface_lists_searches_and_looks_up_by_id():
    index = read(INDEX_RS)
    assert "pub fn search_by_name" in index
    assert "pub fn search_by_mime" in index
    assert "pub fn find_by_id" in index
    syscall = read(SYSCALL_CONTENT_RS)
    assert "INDEX_QUERY_ALL" in syscall
    assert "INDEX_QUERY_NAME" in syscall
    assert "INDEX_QUERY_MIME" in syscall
    assert "INDEX_QUERY_ID" in syscall


def test_the_sdk_wraps_the_same_surface_and_takes_no_slot():
    sdk = read(SDK_CONTENT_RS)
    for call in (
        "pub fn content_index_query",
        "pub fn content_index_search_name",
        "pub fn content_index_search_mime",
        "pub fn content_index_lookup",
        "pub fn intent_resolve",
        "pub fn intent_list",
        "pub fn intent_dispatch",
        "pub fn intent_pending",
    ):
        assert call in sdk, f"the SDK does not expose {call}"
    # The SDK's calls carry the authority implicitly, never a slot number.
    assert "RESERVED_CAP_SLOT" not in sdk


# -----------------------------------------------------------------------------
# The index reads through the filesystem's own path policy
# -----------------------------------------------------------------------------
def test_the_index_reads_through_the_filesystems_path_policy():
    vfs = read(VFS_RS)
    assert "pub fn app_path_visible" in vfs, "the filesystem does not expose its path policy"
    assert "self.enforce_app_path(task_id, path, false).is_ok()" in vfs
    syscall = read(SYSCALL_CONTENT_RS)
    assert "caller_may_see" in syscall
    assert "app_path_visible" in syscall
    # The per-app path policy itself is unchanged.
    assert "fn enforce_app_path" in vfs
    assert '"/storage"' in vfs


def test_the_index_describes_the_shared_storage_area():
    content = read(CONTENT / "mod.rs")
    assert 'SHARED_STORAGE_ROOT: &str = "/storage"' in content
    index = read(INDEX_RS)
    for field in ("pub id:", "pub path:", "pub length:", "pub mime:", "pub provenance:", "pub mtime:"):
        assert field in index, f"a document record is missing {field}"


# -----------------------------------------------------------------------------
# The media type says where it came from
# -----------------------------------------------------------------------------
def test_the_media_type_is_read_from_the_bytes_before_the_name():
    mime = read(MIME_RS)
    classify = mime[mime.index("pub fn classify"):]
    classify = classify[: classify.index("pub fn file_name")]
    bytes_at = classify.index("sniff_bytes")
    extension_at = classify.index("from_extension")
    assert bytes_at < extension_at, "the extension is consulted before the bytes"


def test_the_index_records_the_provenance_of_every_media_type():
    mime = read(MIME_RS)
    assert "pub enum MimeProvenance" in mime
    assert 'MimeProvenance::Bytes => "bytes"' in mime
    assert 'MimeProvenance::Extension => "extension"' in mime
    assert 'MimeProvenance::Unknown => "unknown"' in mime
    index = read(INDEX_RS)
    assert "provenance: MimeProvenance" in index
    # The record carries the provenance out to the wire.
    syscall = read(SYSCALL_CONTENT_RS)
    assert "record.provenance.as_str()" in syscall


def test_the_sdk_reads_the_provenance_back():
    sdk = read(SDK_CONTENT_RS)
    assert "pub provenance: &'a str" in sdk
    assert "pub fn mime_from_bytes" in sdk


# -----------------------------------------------------------------------------
# An intent with no handler is refused, and the refusal names nothing
# -----------------------------------------------------------------------------
def test_the_registry_resolves_nothing_when_nothing_declared():
    intent = read(INTENT_RS)
    resolve = intent[intent.index("pub fn resolve"):]
    resolve = resolve[: resolve.index("pub fn handlers_for")]
    assert "Option<&str>" in resolve
    assert ".find(|handler| handler.kind == kind && handler.value == value)" in resolve


def test_a_refused_intent_is_refused_and_names_nothing():
    syscall = read(SYSCALL_CONTENT_RS)
    resolve = syscall[syscall.index("pub fn sys_intent_resolve"):]
    resolve = resolve[: resolve.index("pub fn sys_intent_list")]
    assert "SyscallResult::TargetNotFound" in resolve, "a refused intent is not refused"
    # The refusal is reported, and it names no package.
    assert "no installed package declares a handler" in resolve.lower()
    assert resolve.count("return Err(SyscallResult::TargetNotFound)") == 1


def test_the_dispatch_of_an_unhandled_intent_is_refused_too():
    syscall = read(SYSCALL_CONTENT_RS)
    dispatch = syscall[syscall.index("pub fn sys_intent_dispatch"):]
    dispatch = dispatch[: dispatch.index("pub fn sys_intent_pending")]
    assert "SyscallResult::TargetNotFound" in dispatch
    assert "no installed package declares a handler" in dispatch.lower()


def test_a_pending_intent_is_only_visible_to_the_package_it_was_dispatched_to():
    intent = read(INTENT_RS)
    assert "pub fn take_pending" in intent
    assert "pub fn peek_pending" in intent
    take = intent[intent.index("pub fn take_pending"):]
    take = take[: take.index("pub fn peek_pending")]
    assert "pending.package_id != package_id" in take
    syscall = read(SYSCALL_CONTENT_RS)
    assert "caller_package_id" in syscall


def test_the_apps_say_that_nothing_handles_a_file():
    """WHY: the platform's answer has to reach the user. An app that guesses a
    handler is worse than an app that says there is none."""
    file_manager = read(APPS / "file_manager" / "src" / "main.rs")
    assert "No app handles this file" in file_manager
    gallery = read(APPS / "gallery" / "src" / "main.rs")
    assert "No app handles this file" in gallery


# -----------------------------------------------------------------------------
# A package's declaration is the only thing that can handle its media type
# -----------------------------------------------------------------------------
def test_the_file_manager_no_longer_launches_through_a_private_table():
    file_manager = read(APPS / "file_manager" / "src" / "main.rs")
    launch = file_manager[file_manager.index("fn open_with_handler"):]
    launch = launch[: launch.index("fn set_as_action")]
    assert "intent_dispatch" in launch
    assert "get_app_for_mime" not in launch
    assert "app_launch" not in launch


def test_the_gallery_hands_a_document_to_the_declared_handler():
    gallery = read(APPS / "gallery" / "src" / "main.rs")
    assert "fn dispatch_open_with" in gallery
    open_with = gallery[gallery.index("fn dispatch_open_with"):]
    open_with = open_with[: open_with.index("impl GalleryApp")]
    assert "intent_resolve" in open_with
    assert "intent_dispatch" in open_with


def test_the_handlers_come_from_the_registry_and_not_from_the_app():
    file_manager = read(APPS / "file_manager" / "src" / "lib.rs")
    assert "pub fn handlers_for_mime" in file_manager
    handlers = file_manager[file_manager.index("pub fn handlers_for_mime"):]
    handlers = handlers[: handlers.index("#[cfg(test)]")]
    assert "intent_list" in handlers, "the handler list is not read from the device's registry"


def test_the_registry_is_built_from_the_packages_manifests():
    package = read(PACKAGE_RS)
    assert "pub const FIRST_PARTY_INTENTS" in package
    assert "pub fn seed_intent_registry" in package
    seed = package[package.index("pub fn seed_intent_registry"):]
    assert "registry.register" in seed
    # The registry is registered when a package is installed and withdrawn when
    # it is uninstalled, so a package that is gone cannot still be the answer.
    syscall = read(KERNEL / "syscall" / "mod.rs")
    assert "registry.register(&verified.package_id, &verified.intents)" in syscall
    assert "registry.unregister(pkg_id)" in syscall


# -----------------------------------------------------------------------------
# The manifest extension, and the packer that accepts it
# -----------------------------------------------------------------------------
def test_the_manifest_can_declare_handlers():
    for app in ("gallery", "music", "browser", "settings"):
        manifest_path = APPS / app / "manifest.json"
        assert manifest_path.is_file(), f"{app} has no manifest to declare its handlers in"
        manifest = json.loads(read(manifest_path))
        assert "intents" in manifest, f"{app} declares no handlers"
        intents = manifest["intents"]
        assert isinstance(intents, dict)
        for kind in intents:
            assert kind in ("mime", "scheme", "action"), f"unknown intent kind {kind}"


def test_the_kernel_table_and_the_manifests_agree():
    """WHY this is checked and not assumed: the first-party applications run as
    kernel-resident images rather than as installed packages, so the kernel
    holds their declarations in a table. The table and the manifests are two
    copies of one fact, and a contract that did not compare them would let
    them drift apart silently."""
    package = read(PACKAGE_RS)
    table = package[package.index("pub const FIRST_PARTY_INTENTS"):]
    table = table[: table.index("];")]
    kernel_pairs = set(
        re.findall(r'\("([^"]+)",\s*"(mime|scheme|action)",\s*"([^"]+)"\)', table)
    )
    assert kernel_pairs, "the kernel's handler table is empty"

    manifest_pairs = set()
    for app_dir in sorted(APPS.iterdir()):
        manifest_path = app_dir / "manifest.json"
        if not manifest_path.is_file():
            continue
        manifest = json.loads(read(manifest_path))
        for kind, values in (manifest.get("intents") or {}).items():
            for value in values:
                manifest_pairs.add((manifest["id"], kind, value))

    assert manifest_pairs == kernel_pairs, (
        "the kernel's handler table and the applications' manifests disagree:\n"
        f"only in the kernel: {sorted(kernel_pairs - manifest_pairs)}\n"
        f"only in the manifests: {sorted(manifest_pairs - kernel_pairs)}"
    )


def test_the_gallery_declares_only_the_images_it_decodes():
    """The gallery lists videos but cannot decode one.

    WHY this is a contract: a manifest is the platform's handler table. A
    gallery that declares video/mp4 becomes the registered handler and can
    shadow the player that would otherwise be asked, while its own handoff
    refuses when it is first. The declaration must match the decoder.
    """
    manifest = json.loads(read(APPS / "gallery" / "manifest.json"))
    mimes = manifest.get("intents", {}).get("mime", [])
    assert mimes, "the gallery declares no handlers at all"
    video = [m for m in mimes if m.startswith("video/")]
    assert video == [], f"the gallery claims video handlers it cannot serve: {video}"
    assert "image/png" in mimes

    source = read(APPS / "gallery" / "src" / "main.rs")
    handoff = source[source.index("fn handoff_video"):]
    handoff = handoff[: handoff.index("\n}\n") + 3]
    assert 'package != "org.interos.gallery"' in handoff, (
        "the gallery would dispatch a video to itself"
    )
    assert "Gallery is the registered handler for " in handoff, (
        "the gallery does not state its refusal to open a video"
    )

    # The player still declares the formats it decodes, so removing the
    # gallery's claim did not remove the real handler.
    player = json.loads(read(APPS / "video_player" / "manifest.json"))
    player_mimes = set(player.get("intents", {}).get("mime", []))
    assert {"video/avi", "video/x-msvideo", "video/x-motion-jpeg"} <= player_mimes


def test_the_packer_encodes_the_declared_handlers():
    interos = read(TOOLS / "interos.py")
    assert "def _rapp_v4_encode" in interos
    assert "def _rapp_intent_table" in interos
    assert "RAPP_FORMAT_VERSION_V4" in read(RAPP_RS)
    assert "RAPP_FORMAT_VERSION_V4: u16 = 4" in read(RAPP_RS)
    # The intent table lives inside the signed section.
    rapp = read(RAPP_RS)
    assert "fn parse_v4_section" in rapp
    assert "fn parse_intent_table" in rapp


def test_the_packer_writes_a_v4_package_the_kernel_can_read():
    """A real round trip: the packer encodes a manifest's declarations and the
    bytes it produces carry the table where the kernel looks for it."""
    spec = importlib.util.spec_from_file_location("interos_tool", TOOLS / "interos.py")
    interos = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(interos)

    manifest = {
        "id": "org.interos.contract",
        "name": "Contract",
        "version": "1.0.0",
        "permissions": ["CAP_LOGGING", "CAP_CONTENT_INDEX", "CAP_CONTENT_INTENT"],
        "intents": {
            "mime": ["image/png"],
            "scheme": ["https"],
            "action": ["org.interos.action.set_wallpaper"],
        },
    }
    payload = b"\x7fELF_CONTRACT_SAMPLE"
    package = interos._rapp_v4_encode(manifest, b"\x01" * 32, b"\0" * 64, payload, "x86_64")

    assert package[:4] == b"RAPP"
    assert int.from_bytes(package[4:6], "little") == 4
    cap_count = int.from_bytes(package[0x4E:0x50], "little")
    section_len = int.from_bytes(package[0xB4:0xB8], "little")
    section_start = 0xB8 + cap_count * 2
    section = package[section_start : section_start + section_len]
    dep_len = int.from_bytes(section[0:4], "little")
    intent_len = int.from_bytes(section[4:8], "little")
    intents = section[8 + dep_len : 8 + dep_len + intent_len]
    assert package[section_start + section_len :] == payload, "the payload moved"

    count = int.from_bytes(intents[0:2], "little")
    assert count == 3
    kinds = []
    cursor = 2
    for _ in range(count):
        kind = intents[cursor]
        length = intents[cursor + 1]
        value = intents[cursor + 2 : cursor + 2 + length].decode("utf-8")
        kinds.append((kind, value))
        cursor += 2 + length
    assert kinds == [
        (0, "image/png"),
        (1, "https"),
        (2, "org.interos.action.set_wallpaper"),
    ]


# -----------------------------------------------------------------------------
# The interlinks, and that they are real
# -----------------------------------------------------------------------------
def test_the_gallery_sets_the_wallpaper_through_the_platform():
    gallery = read(APPS / "gallery" / "src" / "main.rs")
    assert "fn dispatch_wallpaper" in gallery
    dispatch = gallery[gallery.index("fn dispatch_wallpaper"):]
    dispatch = dispatch[: dispatch.index("fn dispatch_open_with")]
    assert "intent_dispatch" in dispatch
    assert "ACTION_SET_WALLPAPER" in gallery
    assert "ACTION_SET_LOCK_WALLPAPER" in gallery
    # The old code built the value and dropped it, so the button did nothing.
    apply = gallery[gallery.index("pub fn apply_wallpaper_target"):]
    apply = apply[: apply.index("pub fn detail_title")]
    assert "dispatch_wallpaper" in apply
    assert "settings_set" not in apply, "the gallery writes the slot behind the platform's back"


def test_the_settings_app_owns_the_wallpaper_and_the_ringtone_slots():
    settings = read(APPS / "settings" / "src" / "main.rs")
    assert "pub fn handle_pending_intent" in settings
    assert "ACTION_SET_WALLPAPER" in settings
    assert "ACTION_SET_LOCK_WALLPAPER" in settings
    assert "ACTION_SET_RINGTONE" in settings
    assert "ui.wallpaper.home" in settings
    assert "ui.wallpaper.home.document" in settings
    assert "ui.wallpaper.lock.document" in settings
    assert "audio.ringtone" in settings
    # The settings app confirms the document against the platform's index
    # before it writes the slot.
    assert "content_index_find_path" in settings


def test_the_launcher_paints_the_wallpaper_the_slot_names():
    launcher = read(APPS / "launcher" / "src" / "main.rs")
    assert "ui.wallpaper.home.document" in launcher
    assert "home_wallpaper_document" in launcher
    assert "std::image::decode_image" in launcher
    home = read(APPS / "launcher" / "src" / "screens" / "home.rs")
    assert "wallpaper_document" in home
    assert "document.pixel(" in home, "the launcher does not paint the document wallpaper"


def test_the_open_with_interlink_reaches_the_declared_handler():
    gallery = read(APPS / "gallery" / "src" / "main.rs")
    assert "fn info_open_with_btn" in gallery
    assert "dispatch_open_with" in gallery
    file_manager = read(APPS / "file_manager" / "src" / "main.rs")
    assert "fn open_with_handler" in file_manager
    assert "intent_dispatch" in file_manager


def test_the_browsers_downloads_land_where_the_index_can_see_them():
    browser = read(APPS / "browser" / "src" / "main.rs")
    assert "fn save_page_to_storage" in browser
    save = browser[browser.index("fn save_page_to_storage"):]
    save = save[: save.index("pub fn navigate(")]
    # WHY the path may be built by a helper: the browser names the folder
    # once, in one place, and the writer uses that name. The contract is
    # that a download lands in the shared storage area, not that the
    # folder's name is spelled out at the call site.
    browser_lib = read(APPS / "browser" / "src" / "lib.rs")
    assert 'DOWNLOAD_PREFIX: &str = "/storage/downloads/' in browser_lib
    assert "download_path(" in save
    assert "write_file(" in save
    # WHY the writer may be a helper: a saved page and a fetched file reach
    # the filesystem through the same chunked writer, and the bytes reach it
    # through fs_write rather than a summary of them.
    writer = browser[browser.index("fn write_file("):]
    writer = writer[: writer.index("impl BrowserApp")]
    assert "std::fs_open" in writer
    assert "std::fs_write" in writer
    # The index covers the whole shared area, so a download is in it.
    content = read(CONTENT / "mod.rs")
    assert 'SHARED_STORAGE_ROOT: &str = "/storage"' in content


def test_the_file_manager_lists_the_documents_the_index_holds():
    file_manager = read(APPS / "file_manager" / "src" / "lib.rs")
    assert "pub fn document_from_index" in file_manager
    assert "content_index_list" in file_manager
    main = read(APPS / "file_manager" / "src" / "main.rs")
    assert "document_from_index" in main


def test_the_gallery_reads_the_same_index():
    gallery = read(APPS / "gallery" / "src" / "main.rs")
    assert "pub fn rescan_storage" in gallery
    rescan = gallery[gallery.index("pub fn rescan_storage"):]
    rescan = rescan[: rescan.index("pub fn load_detail_image")]
    assert "content_index_list" in rescan
    # The gallery no longer scans the storage area with a parser of its own.
    assert "/storage/photos" not in rescan


def test_the_ringtone_slot_is_real_and_the_dialer_reads_it():
    settings = read(APPS / "settings" / "src" / "main.rs")
    assert "audio.ringtone" in settings
    dialer = read(APPS / "dialer" / "src" / "main.rs")
    assert "pub fn refresh_ringtone" in dialer
    refresh = dialer[dialer.index("pub fn refresh_ringtone"):]
    refresh = refresh[: refresh.index("\n    pub fn ", 1)]
    assert "audio.ringtone" in refresh
    assert "content_index_find_path" in refresh
    # The dialer shows the ringtone the slot names.
    assert "ringtone_len" in dialer


def test_the_file_managers_view_options_list_the_handlers_that_exist():
    file_manager = read(APPS / "file_manager" / "src" / "main.rs")
    assert "Open with: " in file_manager
    assert "no app handles this file" in file_manager
    # The options come from the registry, so a handler that is not installed
    # cannot appear in the list.
    assert "document.has_handler()" in file_manager
    assert "document.handlers[..document.handler_count]" in file_manager


def test_the_view_options_are_not_a_guess_written_into_the_app():
    """The old table was `get_app_for_mime`: a private map from a media type
    to an application id. The platform's registry replaced it, and a table
    that can still be reached is a table that can still be used to hand a
    document to a program that never asked for it."""
    lib = read(APPS / "file_manager" / "src" / "lib.rs")
    assert "pub fn get_app_for_mime" not in lib, (
        "the file manager still carries a private media-type-to-application table"
    )
    assert "pub fn default_app_for_mime" not in lib
    main = read(APPS / "file_manager" / "src" / "main.rs")
    assert "get_app_for_mime(" not in main
    assert "default_app_for_mime(" not in main


# -----------------------------------------------------------------------------
# The behaviour, not only the source
# -----------------------------------------------------------------------------
def _cargo_available() -> bool:
    return shutil.which("cargo") is not None


@pytest.mark.skipif(not _cargo_available(), reason="cargo is not on PATH")
def test_the_kernels_own_content_tests_pass():
    """WHY: the rules above are implemented in the kernel, and the kernel's own
    tests are where the behaviour is pinned: a refused intent resolves to
    nothing, the record carries its provenance, and a package's declaration is
    the only thing that handles its media type."""
    result = subprocess.run(
        [
            "cargo",
            "test",
            "--manifest-path",
            str(ROOT / "kernel" / "Cargo.toml"),
            "--lib",
            "content::",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert result.returncode == 0, (
        "the kernel's content tests failed:\n" + result.stdout[-4000:] + result.stderr[-2000:]
    )
    assert "test result: ok" in result.stdout
