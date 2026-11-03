# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from playwright.sync_api import sync_playwright
import time
import sys

def main():
    print("Launching Chromium to test unlock and interaction...")
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={'width': 1920, 'height': 1080})
        page = context.new_page()
        
        page.goto("http://localhost:4300", wait_until="domcontentloaded")
        time.sleep(2.5)
        
        # Click on the canvas to focus
        canvas = page.locator("#live-scanout-canvas")
        box = canvas.bounding_box()
        print("Canvas box:", box)
        
        if box:
            # Swipe up from bottom of phone screen (box['y'] + box['height'] * 0.85) to (box['y'] + box['height'] * 0.2)
            start_x = box['x'] + box['width'] * 0.5
            start_y = box['y'] + box['height'] * 0.85
            end_y = box['y'] + box['height'] * 0.2
            
            print(f"Swiping up from ({start_x}, {start_y}) to ({start_x}, {end_y})...")
            page.mouse.move(start_x, start_y)
            page.mouse.down()
            time.sleep(0.1)
            for step in range(1, 10):
                cur_y = start_y + (end_y - start_y) * (step / 10.0)
                page.mouse.move(start_x, cur_y)
                time.sleep(0.02)
            page.mouse.up()
            
        print("Waiting 2s for unlock animation / frame...")
        time.sleep(2.0)
        
        import os, tempfile
        out_dir = os.environ.get("ARTIFACT_DIR", tempfile.gettempdir())
        out_path = os.path.join(out_dir, "playwright_studio_unlocked.png")
        page.screenshot(path=out_path)
        print(f"Unlocked screenshot saved to: {out_path}")
        browser.close()

if __name__ == "__main__":
    main()
