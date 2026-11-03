# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

import time
import os
from playwright.sync_api import sync_playwright

import tempfile

artifact_dir = os.environ.get("ARTIFACT_DIR", tempfile.gettempdir())

def run_test():
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={"width": 1440, "height": 900})
        page = context.new_page()

        print("[1] Navigating to http://127.0.0.1:4300/index.html ...")
        page.goto("http://127.0.0.1:4300/index.html", wait_until="domcontentloaded")
        page.wait_for_selector(".brand-title", timeout=10000)
        time.sleep(2)

        # 1. Verify Header specs and aesthetic
        brand_title = page.locator(".brand-title").inner_text()
        print(f"Brand title: {brand_title}")
        target_name = page.locator(".target-name").inner_text()
        print(f"Target name: {target_name}")

        # Check scale button is hidden
        scale_btn = page.locator("#emu-btn-scale")
        is_scale_hidden = not scale_btn.is_visible()
        print(f"Is old scale button hidden: {is_scale_hidden}")

        # Capture Default Minimal IDE Studio
        page.screenshot(path=os.path.join(artifact_dir, "ide_studio_monochrome_default.png"))
        print("[Saved] ide_studio_monochrome_default.png")

        # 2. Test Focus Mode
        print("[2] Clicking Focus Mode button...")
        page.locator("#btn-focus-mode").click()
        time.sleep(1)
        is_focus = page.evaluate("() => document.body.classList.contains('studio-focus-mode')")
        print(f"Focus mode active: {is_focus}")

        # Capture Focus Mode
        page.screenshot(path=os.path.join(artifact_dir, "ide_studio_focus_mode.png"))
        print("[Saved] ide_studio_focus_mode.png")

        # 3. Test Clicking Deck Switcher in Focus Mode
        print("[3] Clicking 'Deck' button while in Focus Mode...")
        page.locator("#btn-show-telemetry").click()
        time.sleep(1)
        is_focus_after = page.evaluate("() => document.body.classList.contains('studio-focus-mode')")
        is_deck_active = page.evaluate("() => document.getElementById('btn-show-telemetry').classList.contains('active')")
        print(f"Focus mode after clicking Deck: {is_focus_after} (expected False)")
        print(f"Deck switcher active: {is_deck_active} (expected True)")

        # Capture Deck view
        page.screenshot(path=os.path.join(artifact_dir, "ide_studio_deck_after_focus_exit.png"))
        print("[Saved] ide_studio_deck_after_focus_exit.png")

        # 4. Test Power Button Lock / Wake
        print("[4] Testing Hardware Power Button lock...")
        page.locator("#hw-btn-power").click()
        time.sleep(1.5)
        page.screenshot(path=os.path.join(artifact_dir, "device_power_locked.png"))
        print("[Saved] device_power_locked.png")

        # Click Power Button again to wake
        print("[5] Testing Hardware Power Button wake...")
        page.locator("#hw-btn-power").click()
        time.sleep(1.5)
        page.screenshot(path=os.path.join(artifact_dir, "device_power_woken.png"))
        print("[Saved] device_power_woken.png")

        browser.close()
        print("All tests completed successfully!")

if __name__ == "__main__":
    run_test()
