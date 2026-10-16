#!/usr/bin/env python3
"""Device Studio truth contract: the UI and API must reflect real capability.

Device Studio previously displayed fabricated telemetry (a fake 144 Hz badge,
fake DVFS governor and PMIC readings, a fake baseband carrier, hardcoded CPU
registers, a dead security-audit modal with invented findings) and offered
controls whose endpoints did not exist (Power On, telephony actions, PMIC
profile, KMS flip), reporting success unconditionally. These source-level
contracts pin the corrected behaviour so fabricated status, dead controls, and
unvalidated request handling cannot silently return.

Where a value is genuinely unknown (no telemetry source exists), the required
behaviour is an honest placeholder (Unavailable / No baseband / ...) rather
than an invented number.
"""

from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]


class StudioServerTruthTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")

    def test_every_post_body_length_is_validated_before_read(self):
        # Content-Length is client-controlled; buffering it unbounded lets any
        # local page exhaust memory. One raw parse may remain: the
        # /api/qemu/cmd guard (4096 bytes), pinned by test_studio_security.
        self.assertIn("def _bounded_content_length", self.server)
        self.assertGreaterEqual(self.server.count("self._bounded_content_length()"), 10)
        raw = [
            line for line in self.server.splitlines()
            if "int(self.headers.get('Content-Length'" in line
        ]
        self.assertEqual(
            len(raw), 1,
            f"only the qemu/cmd bounded guard may parse Content-Length directly: {raw}")

    def test_no_wildcard_cors_and_every_verb_is_origin_guarded(self):
        # The Studio API is a local control surface; a wildcard
        # Access-Control-Allow-Origin let any origin read guest telemetry.
        self.assertNotIn("Access-Control-Allow-Origin", self.server)
        self.assertIn("def _origin_allowed", self.server)
        for handler in ("do_GET", "do_POST", "do_OPTIONS"):
            start = self.server.index(f"def {handler}(self):")
            head = self.server[start:start + 400]
            self.assertIn("_origin_allowed()", head,
                          f"{handler} must reject cross-origin requests first")

    def test_proxy_redirects_are_revalidated_against_ssrf_boundary(self):
        # urlopen follows redirects after the initial URL was validated, so a
        # public host could bounce the proxy onto the QEMU monitor.
        self.assertIn("class _SsrfGuardedRedirectHandler", self.server)
        self.assertIn("build_opener(_SsrfGuardedRedirectHandler)", self.server)
        self.assertIn("is_safe_proxy_url(newurl)", self.server)

    def test_frame_cache_purge_targets_the_keys_that_are_written(self):
        # capture_qemu_screen_frame only writes portrait/portrait_jpeg; the
        # old purge popped retired landscape/full keys, so a stale frame
        # survived orientation changes.
        self.assertGreaterEqual(
            self.server.count("_last_frame_cache.pop('portrait_jpeg'"), 2)
        self.assertNotIn("_last_frame_cache.pop('landscape'", self.server)
        self.assertNotIn("_last_frame_cache.pop('full'", self.server)

    def test_power_on_endpoint_really_launches_qemu(self):
        # The Power On button used to POST to a route that did not exist
        # (bare 404). The endpoint must exist, must not spawn a second guest
        # next to a live one, and must report launch_qemu_profile's verdict.
        self.assertIn("'/api/qemu/launch'", self.server)
        self.assertIn("launch_qemu_profile(profile)", self.server)
        self.assertIn("'already_running': True", self.server)

    def test_telephony_actions_answer_409_with_guest_truth(self):
        # The guest registers no baseband backend (kernel/src/telephony/mod.rs),
        # so dial/hangup/data/sms/AT cannot succeed. They must answer 409 with
        # the live guest telemetry instead of a canned success.
        for path in ("/api/telephony/dial", "/api/telephony/hangup",
                     "/api/telephony/data/toggle", "/api/telephony/sms/send",
                     "/api/telephony/at_cmd"):
            self.assertIn(f"'{path}'", self.server)
        self.assertIn("self._send_json(409", self.server)
        self.assertIn("Baseband offline", self.server)
        self.assertIn('extract_guest_subsystem("telephony"', self.server)

    def test_removed_dead_routes_stay_removed(self):
        # No client, tool, test, or document consumed these; they were dead
        # surface area that had to be maintained (and guarded) for nothing.
        for route in ("/api/apps/audit", "/api/fs/list", "/api/kernel/capabilities",
                      "/api/kernel/memory", "/api/kms/status", "/api/qemu/profiles",
                      "/api/qemu/screendump", "/api/screen/stream",
                      "/api/screen/scanout", "/api/themes/current",
                      "/api/touch/status", "/api/transport/metrics"):
            self.assertNotIn(route, self.server, f"dead route returned: {route}")

    def test_removed_reboot_aliases_stay_removed(self):
        # Multiple aliased paths for one action invited divergent behaviour;
        # /api/qemu/reboot and /api/qemu/reset are the supported verbs.
        for route in ("'/api/power'", "'/api/reboot'", "'/api/restart'",
                      "'/api/reset'", "'/api/qemu/restart'",
                      "'/api/hardware/power'"):
            self.assertNotIn(route, self.server, f"alias returned: {route}")

    def test_frame_timing_endpoint_reports_cycles_and_refuses_to_invent_hz(self):
        # WHY: the Studio needs to say what a flip actually cost, and must not
        # convert cycles into a frequency. Cycles have no time base on their own;
        # dividing by a nominal 16.67 ms would print a confident number that no
        # measurement supports. The endpoint therefore returns `refresh_rate_hz:
        # None` explicitly rather than omitting the field or guessing.
        self.assertIn("def extract_frame_timing()", self.server)
        self.assertIn('"refresh_rate_hz": None', self.server)
        self.assertIn("'/api/kernel/frame-timing'", self.server)
        self.assertIn("last_flip_compose_cycles", self.server)
        self.assertIn("last_flip_present_cycles", self.server)
        # An older guest publishes no flip fields; that must read as unknown
        # rather than as a free frame.
        self.assertIn("this is 'unknown', not a zero-cost frame", self.server)

    def test_package_catalog_marks_where_a_real_digest_can_come_from(self):
        # WHY: the Packages tab had a file column and a digest cell that were
        # empty for all 16 packages, including ones the Studio had just written
        # and hashed, because the guest registry line carries neither field.
        # Entries now declare their origin and expose null -- not "" -- so the
        # row can distinguish "embedded, so no host file exists" from "a field
        # failed to arrive".
        self.assertIn("SIDELOAD_LEDGER", self.server)
        self.assertIn('entry["origin"] = "KERNEL_EMBEDDED"', self.server)
        self.assertIn('entry["origin"] = "SIDELOADED_VIA_STUDIO"', self.server)
        # Null, not empty string: the row has to be able to tell "embedded, so
        # no host file exists" from "a field failed to arrive".
        self.assertIn('entry["filename"] = None', self.server)
        self.assertIn('entry["sha256"] = None', self.server)

    def test_guest_vfs_endpoints_declare_guest_authority(self):
        # fs/read and fs/write are the live guest-VFS surface and must state
        # that authority (the old marker lived only on a removed dead route).
        self.assertGreaterEqual(self.server.count("'source': 'GUEST_VFS'"), 2)


class StudioClientTruthTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.js = (ROOT / "emulator/app.js").read_text(encoding="utf-8")

    def test_actions_never_report_success_without_a_server_verdict(self):
        for lie in ("via 3GPP Baseband", "Call terminated (ATH)",
                    "DVFS Governor:", "KMS Page Flip #",
                    "PDP Context Mobile Data toggled"):
            self.assertNotIn(lie, self.js, f"fabricated success toast returned: {lie}")
        for honest in ("d.error || 'Dial rejected'", "d.error || 'Hang-up rejected'",
                       "d.error || 'Data toggle rejected'", "d.error || 'SMS rejected'",
                       "res.error || 'command rejected'"):
            self.assertIn(honest, self.js, f"honest error surfacing missing: {honest}")

    def test_no_client_calls_to_features_that_do_not_exist(self):
        # There is no guest PMIC profile switch and no KMS flip trigger; the
        # buttons and their fetches were removed rather than faked.
        self.assertNotIn("'/api/pmic/profile'", self.js)
        self.assertNotIn("'/api/kms/flip'", self.js)
        self.assertNotIn("cpuHeatmapMatrix", self.js)

    def test_telephony_status_never_renders_undefined(self):
        # Server telemetry has no call_state/operator keys while no baseband
        # is registered; interpolating them showed the literal word undefined.
        self.assertIn("t.call_state ||", self.js)
        self.assertIn("t.operator ||", self.js)
        self.assertNotIn("textContent = t.call_state;", self.js)
        self.assertNotIn("textContent = t.operator;", self.js)

    def test_package_table_reports_real_counts_and_neutral_fallbacks(self):
        self.assertIn("pkg-count-badge", self.js)
        self.assertIn("'Status unavailable'", self.js)
        self.assertNotIn("'Verified (Ed25519)'", self.js)
        # WHY the digest fallback is no longer the bare string "SHA-256
        # unavailable": that cell was saying the same thing for all 16 packages,
        # including ones this Studio had just written to disk and hashed. A
        # kernel-embedded package has no host file to digest, which is a
        # different fact from "a digest was expected and did not arrive", and a
        # package this Studio sideloaded has a real one. The row now says which.
        self.assertIn("'No host file to digest'", self.js)
        self.assertIn("'Digest not reported'", self.js)
        self.assertIn("'Embedded in the guest image'", self.js)
        self.assertNotIn("'SHA-256 unavailable'", self.js)

    def test_package_table_merges_guest_rows_instead_of_overwriting_them(self):
        # WHY: `state.installedPackages = data.apps` replaced the whole array
        # with the guest registry, which carries no filename and no digest. A
        # package sideloaded seconds earlier lost both fields and rendered as
        # "SHA-256 unavailable" on the next 4-second refresh.
        self.assertNotIn("state.installedPackages = data.apps;", self.js)
        self.assertIn("byId.set(incoming.id, prior ? { ...prior, ...incoming } : incoming)", self.js)

    def test_studio_reports_measured_flip_cost_separately_from_the_display_mode(self):
        # WHY the client half of the DRM/KMS correction: the panel showed only
        # the programmed mode's 60 Hz, which read as achieved frame rate. It now
        # also reports what a flip actually cost, and says explicitly that it is
        # not a frame rate -- the guest exports no per-frame wall-clock interval,
        # so one cannot be derived without inventing it.
        self.assertIn("/api/kernel/frame-timing", self.js)
        self.assertIn("Not a frame rate", self.js)
        self.assertIn("'Not measured'", self.js)

    def test_sideload_log_does_not_overstate_what_was_verified(self):
        # WHY: "SHA-256 Verified" after a sideload read as "the guest trusts
        # this publisher". What happened is that the Studio hashed the bytes it
        # received. The server says "guest trust remains authoritative".
        self.assertNotIn("(SHA-256 Verified)", self.js)
        self.assertIn("guest trust verification is the guest's decision", self.js)

    def test_process_stats_are_driven_by_live_data(self):
        self.assertIn("statActiveTasks", self.js)
        self.assertIn("statActiveTasks.textContent", self.js)
        self.assertIn("procCountBadge", self.js)


class StudioMarkupTruthTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.html = (ROOT / "emulator/index.html").read_text(encoding="utf-8")

    def test_drm_tab_separates_the_display_mode_from_the_measured_flip_cost(self):
        # WHY two cards. `display_kms.rs` programs the scanout at refresh_hz = 60
        # and derives ticks_per_frame from it, so 60 Hz is a real parameter of
        # the emulated mode. It is not a claim about achieved frame rate, and
        # the guest does not achieve it. The panel used to show only the mode,
        # which read as a performance claim; it now shows the mode AND what a
        # flip actually costs, as two different facts.
        self.assertIn("DISPLAY MODE REFRESH", self.html)
        self.assertIn("60 Hz VSYNC", self.html)
        self.assertIn("refresh_hz = 60", self.html)
        self.assertIn("MEASURED FLIP COST", self.html)
        self.assertIn('id="kms-refresh"', self.html)
        self.assertIn('id="kms-refresh-note"', self.html)
        # LTPO/VRR were never a mode parameter and stay gone.
        self.assertNotIn("LTPO 120Hz VRR", self.html)
        self.assertIn("No live compositor-plane telemetry is currently exposed by the guest.",
                      self.html)

    def test_fabricated_telemetry_is_gone(self):
        for lie in (
            "144 Hz", "Ultra Fluid",                       # fake FPS badge
            "0x000001000000DC4D", "Ring 0 Safe", "TID: Active",  # fake registers
            "1280&times;720", "1280x720",                  # wrong scanout geometry
            "InterCell", "5G SA",                          # fake carrier/signal
            "4210 mV", "31.8 &deg;C", "1600 MHz", "-380 mA", "96% SoC",  # fake PMIC
            "dvfs-btn", "Trigger Hardware Page Flip", "3120 Page Flips",
            "0xFD800000",                                 # fake KMS controls/state
            "8 Processes", "0.42 &mu;s",                   # fake allocator stats
            "9 Packages Active", "Live Scheduler",         # fake counts
            "org.fossify.filemanager", "modal-strict-audit",
            "APK Scheme v2/v3 Verified",                  # dead fabricated audit modal
            "ENABLED (IPv4/IPv6)", "app:music",           # fake PDP state, fake IPC rows
            "Intel HDA / PulseAudio", "/data/apps/",      # wrong capability facts
            "Live Heartbeat", "5 Contact Slots Active",
        ):
            self.assertNotIn(lie, self.html, f"fabricated markup returned: {lie}")

    def test_truthful_placeholders_and_facts_are_present(self):
        for fact in (
            "PIT: 100 Hz",            # real PIT frequency (hal/platform.rs)
            "IRQ 32..47",             # real PIC offsets (PIC_1_OFFSET = 32)
            "COM1 0x3F8",             # real serial port
            "Double-Buffered", "60 Hz VSYNC",  # real KMS refresh_hz = 60
            "720&times;1440 scanout",         # real QEMU EDID geometry
            "Measuring FPS",                 # pending state until measured
            "No baseband",                   # honest baseband state
            "Unavailable",                   # honest PMIC state
            "5 contact slots supported",     # real MAX_TOUCH_SLOTS = 5, capacity wording
            "No live compositor-plane telemetry",  # honest empty state
        ):
            self.assertIn(fact, self.html, f"truthful content missing: {fact}")

    def test_register_grid_starts_as_pending_not_invented(self):
        # Static hex seeds claimed register values before any monitor poll;
        # JS fills them from `info registers` (or shows an error).
        self.assertIn('id="reg-rip">&hellip;', self.html)
        self.assertIn('id="reg-rsp">&hellip;', self.html)
        self.assertNotIn(">0x000001000000", self.html)

    def test_audit_modal_markup_is_removed(self):
        # The opener was removed as synthetic telemetry in 99e50b7 but the
        # hidden modal (invented package findings, "Verified" signature claim
        # the server explicitly does not perform) was left behind.
        self.assertNotIn("modal-strict-audit", self.html)
        self.assertNotIn("audit-pkg-sig", self.html)
        self.assertNotIn("btn-modal-launch-app", self.html)


if __name__ == "__main__":
    unittest.main()
