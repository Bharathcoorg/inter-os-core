# Inter OS Application Development Guide (For Non-Rust Developers)

Welcome to application development on **Inter OS**! You do **not** need to know Rust to build world-class, sovereign mobile applications.

---

## 1. Application Architecture

An Inter OS web-standard application is a lightweight, sandboxed package composed of standard web technologies:

```
my_app/
├── app.json             # Declarative package manifest (id, permissions, display)
├── index.html           # Application UI markup
├── style.css            # InterOS Glassmorphism styling & theme tokens
├── app.js               # Application business logic
└── interos.js           # Official InterOS Universal SDK client bridge
```

---

## 2. Declarative `app.json` Manifest

Every application declares its metadata and security permissions in `app.json`:

```json
{
  "id": "org.interos.notes",
  "name": "Sovereign Notes",
  "version": "1.0.0",
  "description": "Minimalist encrypted scratchpad built with HTML, CSS & JavaScript",
  "author": "Community Developer",
  "entry": "index.html",
  "icon": "📝",
  "permissions": [
    "vfs.read",
    "vfs.write",
    "notifications",
    "audio.feedback",
    "telemetry.battery"
  ]
}
```

### Supported Capability Permissions:
- `vfs.read`: Read sandboxed files from the virtual filesystem (`/data/user/`).
- `vfs.write`: Write and persist data to sandboxed storage.
- `telemetry.battery`: Query hardware fuel gauge (battery percentage, charging status).
- `telemetry.sensors`: Query accelerometer, gyroscope, and ambient light sensors.
- `notifications`: Dispatch system toast alerts and head-up notifications.
- `audio.feedback`: Trigger haptic and audio feedback beeps.

---

## 3. The `InterOS` JavaScript SDK

Include `interos.js` in your `index.html`. It exposes the global `window.InterOS` namespace:

### Virtual Filesystem (VFS)
```javascript
// Read a file
const text = await InterOS.vfs.readFile('/data/user/my_document.txt');

// Write a file
await InterOS.vfs.writeFile('/data/user/my_document.txt', 'Hello, Inter OS!');

// List directory contents
const dir = await InterOS.vfs.list('/data/user');
```

### Hardware Telemetry
```javascript
// Query battery status
const batt = await InterOS.telemetry.getBattery();
console.log(`Battery: ${batt.level}%, Charging: ${batt.charging}`);

// Query SoC thermals
const therm = await InterOS.telemetry.getThermals();
console.log(`SoC Temp: ${therm.socTempC}°C`);
```

### Audio & Notifications
```javascript
// Play an audio feedback tone (frequency in Hz, duration in seconds)
InterOS.audio.beep(800, 0.05, 'sine');

// Display a system toast notification
InterOS.notifications.show('Document saved successfully!');
```

---

## 4. Testing & Running in Device Studio

1. Start the Inter OS Device Studio:
   ```cmd
   run_emulator.bat
   ```
2. Open `http://localhost:4300` in your browser.
3. In the Device Studio side panel, drag and drop your application folder or zip file into the **Package Sideload** tray.
4. Your application will appear immediately on the smartphone home screen and dock!
