# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from playwright.sync_api import sync_playwright
import time
import sys

def main():
    out_path = sys.argv[1] if len(sys.argv) > 1 else "playwright_studio.png"
    print(f"Launching Chromium and navigating to http://localhost:4300...")
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={'width': 1920, 'height': 1080})
        page = context.new_page()
        
        console_logs = []
        page.on("console", lambda msg: console_logs.append(f"[{msg.type}] {msg.text}"))
        page.on("pageerror", lambda err: console_logs.append(f"[ERROR] {err}"))
        
        page.goto("http://localhost:4300", wait_until="domcontentloaded")
        print("Page loaded (DOMContentLoaded), waiting 3.5 seconds for canvas frames...")
        time.sleep(3.5)
        
        # Check canvas and badge
        spec_badge = page.evaluate("() => document.getElementById('spec-display')?.textContent")
        canvas_info = page.evaluate("() => { const el = document.getElementById('live-scanout-canvas'); return el ? {w: el.width, h: el.height, display: el.style.display} : null; }")
        splash_info = page.evaluate("() => { const el = document.getElementById('phone-boot-splash'); return el ? {visible: el.classList.contains('is-visible'), display: window.getComputedStyle(el).display} : 'absent'; }")
        global_status = page.evaluate("() => document.getElementById('qemu-global-status')?.textContent?.trim()")
        
        print(f"Global Status: {global_status}")
        print(f"Spec Display: {spec_badge}")
        print(f"Canvas Info: {canvas_info}")
        print(f"Splash Info: {splash_info}")
        
        page.screenshot(path=out_path)
        print(f"Screenshot saved to {out_path}")
        browser.close()
        
        if console_logs:
            print(f"\nBrowser console logs ({len(console_logs)} total):")
            for log in console_logs[-20:]:
                print("  ", log)

if __name__ == "__main__":
    main()
