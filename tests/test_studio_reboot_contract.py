#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""Device Studio reboot contract: a reboot has to be a real, observed reboot.

The reboot route used to answer "rebooted" the moment QEMU's monitor accepted
`system_reset`. That command resets the machine's devices even when the
accelerator does not restart the guest: under WHPX the CPU kept running while
the COM1 UART was reset, so the Studio's console died without anything
rebooting, and the route reported a device state it had never observed. The
same reset also abandons the chardev client instead of closing it, so the
guest's new boot log went into a socket nobody served.

These contracts pin the corrected behaviour: the reboot is requested from the
guest's own kernel, the orphaned console client is reclaimed, the route waits
for the new launcher before it claims success, and the API reports the
mechanism it actually used.
"""

from pathlib import Path
import sys
import threading
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from emulator import server as studio  # noqa: E402

LAUNCHER_LINE = '[LAUNCHER] Step 5: Initial presentation complete.'
ACK_LINE = '[SYSTEM] System reboot requested via COM1 serial.'


class StudioRebootSourceContractTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = (ROOT / "emulator/server.py").read_text(encoding="utf-8")
        cls.app = (ROOT / "emulator/app.js").read_text(encoding="utf-8")

    def test_reboot_route_uses_the_observed_reboot_helper(self):
        # The route must not answer from the monitor's acknowledgement alone.
        route = self.server[self.server.index("if self.path in ('/api/qemu/reboot'"):]
        route = route[:route.index("elif action in ('sleep', 'power', 'wake', 'toggle')")]
        self.assertIn("perform_warm_reboot()", route)
        self.assertNotIn("send_qemu_monitor_cmd('system_reset')", route,
                         "the warm reboot must not be a bare monitor reset")

    def test_warm_reboot_prefers_the_guests_own_reboot_request(self):
        helper = self.server[self.server.index("def perform_warm_reboot():"):]
        helper = helper[:helper.index("def await_guest_reboot(")]
        guest_at = helper.index("request_guest_reboot()")
        monitor_at = helper.index("send_qemu_monitor_cmd('system_reset')")
        self.assertLess(guest_at, monitor_at,
                        "the guest kernel's reboot request must be tried first")
        self.assertIn("restart_method = 'guest_reboot'", helper)
        self.assertIn("restart_method = 'monitor_system_reset'", helper)

    def test_warm_reboot_reclaims_the_console_and_waits_for_the_new_boot(self):
        helper = self.server[self.server.index("def perform_warm_reboot():"):]
        helper = helper[:helper.index("def await_guest_reboot(")]
        self.assertIn("release_stale_serial_client(", helper)
        self.assertIn("reset_monitor_connection()", helper)
        self.assertIn("await_guest_reboot()", helper)

    def test_serial_worker_has_a_console_liveness_watchdog(self):
        # A reset abandons the chardev client without closing it, so EOF never
        # arrives and the reader would wait on a console that is gone.
        self.assertIn("SERIAL_CONSOLE_SILENCE_LIMIT", self.server)
        self.assertIn("SERIAL_CONSOLE_BOOT_GRACE", self.server)
        worker = self.server[self.server.index("def serial_stream_worker():"):]
        worker = worker[:worker.index("def release_stale_serial_client(")]
        self.assertIn("time.time() - last_rx > limit", worker)
        self.assertIn("guest_spoke", worker)

    def test_reboot_response_reports_the_observed_fact_and_the_mechanism(self):
        response = self.server[self.server.index("'guest_rebooted': bool(ok)"):]
        response = response[:response.index("}).encode())")]
        self.assertIn("'guest_rebooted': bool(ok)", response)
        self.assertIn("'restart_method': restart_method if ok else 'failed'", response)
        self.assertNotIn("'qemu_reset':", response,
                         "a flag that only the monitor path could set would read as "
                         "\"the guest did not reboot\" for a guest-level reboot")

    def test_ui_names_the_mechanism_it_reports(self):
        self.assertIn("warmMethod === 'guest_reboot'", self.app)
        self.assertNotIn("showToast('Guest reset via QEMU monitor (not a cold restart)')", self.app)


class StudioRebootBehaviourTests(unittest.TestCase):
    """The helpers have to behave, not merely exist."""

    def setUp(self):
        self._serial = list(studio.serial_lines)
        self._cursor = studio.last_reboot_cursor
        self._sender = studio.send_guest_serial_command
        studio.serial_lines.clear()
        studio.last_reboot_cursor = 0

    def tearDown(self):
        studio.serial_lines.clear()
        studio.serial_lines.extend(self._serial)
        studio.last_reboot_cursor = self._cursor
        studio.send_guest_serial_command = self._sender

    def test_guest_acknowledgement_is_accepted_as_the_reboot_request(self):
        def fake_send(command, wait_for=None, timeout=12):
            self.assertEqual(command, "REBOOT")
            with studio.telemetry_lock:
                studio.serial_lines.append(ACK_LINE)
            return True, "sent"

        studio.send_guest_serial_command = fake_send
        accepted, detail = studio.request_guest_reboot(timeout=5.0)
        self.assertTrue(accepted, detail)

    def test_a_guest_that_never_answers_is_not_a_reboot(self):
        studio.send_guest_serial_command = lambda command, wait_for=None, timeout=12: (True, "sent")
        accepted, detail = studio.request_guest_reboot(timeout=1.0)
        self.assertFalse(accepted)
        self.assertIn("did not acknowledge", detail)

    def test_a_dead_console_is_not_a_reboot(self):
        studio.send_guest_serial_command = lambda command, wait_for=None, timeout=12: (
            False, "guest serial transport unavailable")
        accepted, detail = studio.request_guest_reboot(timeout=1.0)
        self.assertFalse(accepted)
        self.assertIn("transport unavailable", detail)

    def test_the_new_launcher_is_the_reboot_evidence(self):
        def publish():
            time.sleep(0.4)
            with studio.telemetry_lock:
                studio.serial_lines.append(LAUNCHER_LINE)

        threading.Thread(target=publish, daemon=True).start()
        rebooted, detail = studio.await_guest_reboot(timeout=10.0, rotate_after=30.0)
        self.assertTrue(rebooted, detail)

    def test_a_guest_that_never_comes_back_is_not_a_reboot(self):
        rebooted, detail = studio.await_guest_reboot(timeout=1.0, rotate_after=30.0)
        self.assertFalse(rebooted)
        self.assertIn("no new boot output", detail)


if __name__ == "__main__":
    unittest.main()
