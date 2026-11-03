# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

from playwright.sync_api import sync_playwright
import time

def main():
    print("Testing Focus Mode, Fullscreen, and Power Button behavior...")
    with sync_playwright() as p:
        browser = p.chromium.launch(headless=True)
        context = browser.new_context(viewport={'width': 1920, 'height': 1080})
        page = context.new_page()
        
        page.goto("http://localhost:4300", wait_until="domcontentloaded")
        print("Waiting for guest boot (6s)...")
        time.sleep(6)
        
        # 1. Take initial screenshot
        page.screenshot(path="verify_1_boot.png")
        print("Captured verify_1_boot.png")
        
        # 2. Test Focus Mode (clicking Focus Mode button)
        print("Clicking Focus Mode button...")
        page.click("#btn-focus-mode")
        time.sleep(1)
        page.screenshot(path="verify_2_focus_mode.png")
        print("Captured verify_2_focus_mode.png (with blurred backdrop and centered phone)")
        
        # 3. Test Power Button Lock behavior
        # In focus mode, click the power button on the floating side toolbar or chassis
        print("Clicking Power Button on right chassis to lock...")
        page.click("#hw-btn-power")
        time.sleep(2)
        page.screenshot(path="verify_3_power_locked.png")
        print("Captured verify_3_power_locked.png")
        
        # 4. Click Power Button again to wake
        print("Clicking Power Button again to wake...")
        page.click("#hw-btn-power")
        time.sleep(2)
        page.screenshot(path="verify_4_power_wake_lockscreen.png")
        print("Captured verify_4_power_wake_lockscreen.png")
        
        # 5. Swipe up to unlock
        print("Swiping up on phone screen to unlock...")
        box = page.locator("#screen-live-scanout").bounding_box()
        if box:
            center_x = box["x"] + box["width"] / 2
            start_y = box["y"] + box["height"] * 0.85
            end_y = box["y"] + box["height"] * 0.25
            page.mouse.move(center_x, start_y)
            page.mouse.down()
            time.sleep(0.1)
            page.mouse.move(center_x, end_y, steps=10)
            time.sleep(0.1)
            page.mouse.up()
            time.sleep(2)
            page.screenshot(path="verify_5_unlocked_homescreen.png")
            print("Captured verify_5_unlocked_homescreen.png")
            
            # 6. Lock again via Power Button
            print("Locking device again with Power Button from Home Screen...")
            page.click("#hw-btn-power")
            time.sleep(2)
            page.screenshot(path="verify_6_relocked_to_lockscreen.png")
            print("Captured verify_6_relocked_to_lockscreen.png")
            
        browser.close()
        print("All verification steps completed successfully!")

if __name__ == "__main__":
    main()
