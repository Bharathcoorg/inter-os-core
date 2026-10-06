# InterOS Web App Template (Device Studio Prototype)

> **Status Notice: Experimental Web Prototype Template**  
> This template is an experimental client-side web application designed to run inside **Device Studio** (`http://localhost:4300`).  
> **Native InterOS Applications**: Native production applications for InterOS Core execute directly as bare-metal ELF binaries within isolated Ring-3 address spaces using the cryptographically signed `.rapp` package format. Web apps run as sandboxed web prototypes interacting with the Device Studio API server.

---

## 1. Directory Structure

```text
web_app_template/
├── app.json             # Manifest metadata (id, name, permissions)
├── index.html           # UI markup
├── style.css            # Canonical neutral design styling
├── app.js               # Application client logic
├── interos.js           # Client SDK bridge communicating with Device Studio
└── README.md            # This manual
```

---

## 2. Declarative `app.json` Manifest

Web app metadata and declared permissions are configured in `app.json`:

```json
{
  "id": "org.interos.notes",
  "name": "Sovereign Notes",
  "version": "1.0.0",
  "description": "Minimalist scratchpad built with HTML, CSS & JavaScript",
  "author": "Community Developer",
  "entry": "index.html",
  "icon": "📝",
  "permissions": [
    "vfs.read",
    "vfs.write",
    "telemetry.battery",
    "notifications",
    "audio.feedback"
  ]
}
```

---

## 3. The `InterOS` Client JavaScript SDK (`interos.js`)

Including `interos.js` exposes the global `window.InterOS` client bridge to Device Studio HTTP endpoints:

### Virtual Filesystem (`InterOS.vfs`)
Interacts with the sandboxed `/api/fs/` endpoints:
```javascript
// List files in a directory:
const files = await InterOS.vfs.list('/data/user');

// Read a text file:
const content = await InterOS.vfs.readFile('/data/user/notes.txt');

// Write a text file:
const bytesWritten = await InterOS.vfs.writeFile('/data/user/notes.txt', 'Hello InterOS');

// Delete a file:
await InterOS.vfs.deleteFile('/data/user/notes.txt');
```

### Telemetry (`InterOS.telemetry`)
Queries the running emulator state from `/api/qemu/telemetry`:
```javascript
// Query battery fuel gauge status:
const battery = await InterOS.telemetry.getBattery();
console.log(`Battery: ${battery.level}%, Charging: ${battery.charging}`);

// Query SoC temperature:
const thermals = await InterOS.telemetry.getThermals();
console.log(`SoC Temperature: ${thermals.socTempC}°C`);
```

### Audio Feedback (`InterOS.audio`)
Synthesizes short tones using the Web Audio API:
```javascript
// Beep tone (frequency in Hz, duration in seconds, waveform type):
InterOS.audio.beep(520, 0.05, 'sine');
```

### Notifications (`InterOS.notifications`)
Displays a visual toast alert in Device Studio:
```javascript
InterOS.notifications.show('Document saved successfully!', 'info');
```

---

## 4. Running in Device Studio

1. Start Device Studio:
   ```bash
   python emulator/server.py
   ```
2. Navigate to `http://localhost:4300` in your web browser.
3. Sideload the application directory via the Device Studio developer panel.
