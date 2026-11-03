# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

import sys
import time
import subprocess
import urllib.request
import json

def capture():
    # Start Chrome with remote debugging
    chrome_path = r"C:\Program Files\Google\Chrome\Application\chrome.exe"
    out_png = sys.argv[1] if len(sys.argv) > 1 else "studio_capture.png"
    
    proc = subprocess.Popen([
        chrome_path,
        "--headless=new",
        "--disable-gpu",
        "--remote-debugging-port=9333",
        "--window-size=1920,1080",
        "http://localhost:4300"
    ])
    
    try:
        # Wait for debugging port
        ws_url = None
        for _ in range(20):
            time.sleep(0.5)
            try:
                resp = urllib.request.urlopen("http://127.0.0.1:9333/json", timeout=1)
                tabs = json.loads(resp.read().decode())
                if tabs and "webSocketDebuggerUrl" in tabs[0]:
                    ws_url = tabs[0]["webSocketDebuggerUrl"]
                    break
            except Exception:
                pass
                
        if not ws_url:
            print("Failed to get debugger URL")
            return False
            
        print("Connected to Chrome, waiting 3s for live scanout frame...")
        time.sleep(3.0)
        
        # Use python websocket or simple command to capture screenshot
        # Or let Chrome take screenshot directly after page has loaded
        import base64
        import socket
        
        # Simple WebSocket client without external deps
        # Let's import urllib or use python standard library
    finally:
        try:
            proc.terminate()
            proc.wait(timeout=2)
        except Exception:
            pass

if __name__ == "__main__":
    capture()
