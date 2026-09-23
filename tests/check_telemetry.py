import urllib.request
import json
import time

# Trigger reboot
req = urllib.request.Request("http://localhost:4200/api/qemu/reboot", data=b"{}", headers={'Content-Type': 'application/json'})
try:
    with urllib.request.urlopen(req) as resp:
        print("Reboot triggered:", resp.read().decode())
except Exception as e:
    print("Reboot error:", e)

time.sleep(3.5)

try:
    with urllib.request.urlopen("http://localhost:4200/api/qemu/telemetry") as resp:
        data = json.loads(resp.read().decode())
        print("Total tests passed:", len(data.get('kernel', {}).get('tests_passed', [])))
        for t in data.get('kernel', {}).get('tests_passed', []):
            if any(k in t for k in ['SDHCI', 'Goodix', 'SimpleFB', 'I2C', 'PSCI', 'FDT']):
                print(" ->", t)
except Exception as e:
    print("Telemetry error:", e)
