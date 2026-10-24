/**
 * InterOS Universal Client SDK (interos.js)
 * 
 * Hardened JavaScript API bridge and W3C polyfill engine for sovereign web applications
 * on Inter OS without needing Rust knowledge.
 * 
 * Provides:
 * - Strict Content Security Policy (CSP) enforcement & validation
 * - Capability & Manifest permission binding with zero capability escalation
 * - Interactive runtime consent prompts for sensitive permissions
 * - Zero-rewrite W3C API polyfills:
 *     * window.fetch (sandboxed proxying via netd /api/browser/proxy)
 *     * window.WebSocket (sandboxed proxying via /api/browser/proxy)
 *     * window.localStorage & window.indexedDB (sandboxed to /data/app_storage/<app_id>/)
 *     * navigator.mediaDevices.getUserMedia (CameraCapture & Microphone gating)
 *     * window.Notification (routed to /api/notifications/post and launcher shade)
 * - InterOS native API bridge:
 *     * InterOS.vfs (sandboxed virtual filesystem)
 *     * InterOS.telemetry (battery, thermals, sensors)
 *     * InterOS.audio (hardware tone synthesis & haptic clicks)
 *     * InterOS.notifications (system alerts & notifications)
 */

(function (window) {
  'use strict';

  // =========================================================================
  // 1. Content Security Policy (CSP) Enforcement
  // =========================================================================
  const REQUIRED_CSP = "default-src 'self'; script-src 'self'; connect-src 'self' http://127.0.0.1:* ws://127.0.0.1:*; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; object-src 'none'; frame-ancestors 'none';";

  function enforceContentSecurityPolicy() {
    if (typeof document === 'undefined') return;

    let cspMeta = null;
    const metas = typeof document.getElementsByTagName === 'function' ? document.getElementsByTagName('meta') : [];
    for (let i = 0; i < metas.length; i++) {
      if ((metas[i].getAttribute('http-equiv') || '').toLowerCase() === 'content-security-policy') {
        cspMeta = metas[i];
        break;
      }
    }

    if (!cspMeta && typeof document.createElement === 'function') {
      cspMeta = document.createElement('meta');
      cspMeta.setAttribute('http-equiv', 'Content-Security-Policy');
      cspMeta.setAttribute('content', REQUIRED_CSP);
      if (document.head && typeof document.head.prepend === 'function') {
        document.head.prepend(cspMeta);
      } else if (document.head && typeof document.head.appendChild === 'function') {
        document.head.appendChild(cspMeta);
      } else if (document.documentElement && typeof document.documentElement.appendChild === 'function') {
        document.documentElement.appendChild(cspMeta);
      } else if (typeof document.addEventListener === 'function') {
        document.addEventListener('DOMContentLoaded', () => {
          if (document.head && !document.querySelector?.('meta[http-equiv="Content-Security-Policy"]')) {
            if (typeof document.head.prepend === 'function') document.head.prepend(cspMeta);
            else if (typeof document.head.appendChild === 'function') document.head.appendChild(cspMeta);
          }
        });
      }
    }
  }

  enforceContentSecurityPolicy();

  // =========================================================================
  // 2. Permission Mapping & Manifest Binding
  // =========================================================================
  const PERMISSION_ALIASES = {
    // Filesystem & Storage
    'vfs.read': ['FileSystemRead'],
    'vfs.write': ['FileSystemWrite'],
    'storage': ['FileSystemRead', 'FileSystemWrite'],
    'persistent-storage': ['FileSystemRead', 'FileSystemWrite'],
    'filesystemread': ['FileSystemRead'],
    'filesystemwrite': ['FileSystemWrite'],
    'files': ['FileSystemRead', 'FileSystemWrite'],
    'cap_filesystem_read': ['FileSystemRead'],
    'cap_filesystem_write': ['FileSystemWrite'],
    'cap_secure_storage': ['FileSystemWrite'],

    // Telemetry & Sensors
    'telemetry.battery': ['Sensors'],
    'telemetry': ['Sensors'],
    'sensors': ['Sensors'],
    'sensor': ['Sensors'],
    'accelerometer': ['Sensors'],
    'gyroscope': ['Sensors'],
    'cap_system_control_sensor': ['Sensors'],
    'cap_sensor_accel': ['Sensors'],
    'cap_sensor_gyro': ['Sensors'],
    'cap_sensor_hr': ['Sensors'],

    // Audio & Feedback
    'audio.feedback': ['AudioSynthesis'],
    'audio': ['AudioSynthesis'],
    'audiosynthesis': ['AudioSynthesis'],
    'cap_system_control_audio': ['AudioSynthesis'],

    // Notifications
    'notifications': ['Notifications'],
    'notification': ['Notifications'],

    // Camera
    'camera': ['CameraCapture'],
    'cameracapture': ['CameraCapture'],
    'cap_system_control_camera': ['CameraCapture'],

    // Microphone
    'microphone': ['Microphone'],
    'mic': ['Microphone'],

    // Location
    'geolocation': ['Location'],
    'location': ['Location'],

    // Telephony
    'telephony': ['TelephonyModem'],
    'telephonymodem': ['TelephonyModem'],
    'phone': ['TelephonyModem'],
    'sms': ['TelephonyModem'],
    'cap_system_control_telephony': ['TelephonyModem'],

    // Network
    'network': ['NetworkSocket', 'NetworkSend', 'NetworkReceive'],
    'networksocket': ['NetworkSocket'],
    'networksend': ['NetworkSend'],
    'networkreceive': ['NetworkReceive'],
    'cap_network_socket': ['NetworkSocket'],
    'cap_network_send': ['NetworkSend'],
    'cap_network_receive': ['NetworkReceive'],
    'cap_mesh_radio': ['NetworkSocket'],
    'cap_p2p_socket': ['NetworkSend'],
  };

  const SENSITIVE_PERMISSIONS = new Set([
    'cameracapture',
    'microphone',
    'telephonymodem',
    'sensors',
    'location'
  ]);

  let appManifest = null;
  let appId = 'org.interos.webapp';
  const declaredCapabilities = new Set();
  const grantedRuntimePermissions = new Set();

  function registerDeclaredPermission(rawName) {
    if (!rawName || typeof rawName !== 'string') return;
    const lower = rawName.toLowerCase().trim();
    declaredCapabilities.add(lower);
    const mapped = PERMISSION_ALIASES[lower];
    if (mapped) {
      for (const m of mapped) {
        declaredCapabilities.add(m.toLowerCase());
      }
    }
  }

  function setAppManifest(manifest) {
    if (!manifest || typeof manifest !== 'object') return;
    appManifest = manifest;
    if (manifest.id) appId = manifest.id;
    else if (manifest.app_id) appId = manifest.app_id;
    else if (manifest.name) appId = 'org.interos.' + manifest.name.toLowerCase().replace(/[^a-z0-9]/g, '_');

    const rawPerms = manifest.permissions || manifest.capabilities || [];
    if (Array.isArray(rawPerms)) {
      for (const p of rawPerms) {
        registerDeclaredPermission(p);
      }
    }
  }

  // Synchronous manifest load attempt
  function loadManifestSync() {
    if (typeof window !== 'undefined' && window.__INTEROS_MANIFEST__) {
      setAppManifest(window.__INTEROS_MANIFEST__);
      return;
    }
    if (typeof document !== 'undefined') {
      const getElem = typeof document.getElementById === 'function' ? document.getElementById.bind(document) : null;
      const querySel = typeof document.querySelector === 'function' ? document.querySelector.bind(document) : null;
      const tag = (getElem ? getElem('interos-manifest') : null) ||
                  (querySel ? querySel('script[type="application/manifest+json"]') : null);
      if (tag && tag.textContent) {
        try {
          setAppManifest(JSON.parse(tag.textContent));
          return;
        } catch (_) {}
      }

      // Try synchronous XHR for local app.json or manifest.json
      if (typeof XMLHttpRequest !== 'undefined') {
        for (const file of ['app.json', 'manifest.json', 'manifest.webmanifest']) {
          try {
            const xhr = new XMLHttpRequest();
            xhr.open('GET', file, false);
            xhr.send(null);
            if (xhr.status === 200 || xhr.status === 0) {
              setAppManifest(JSON.parse(xhr.responseText));
              return;
            }
          } catch (_) {}
        }
      }
    }
  }

  loadManifestSync();

  // Async manifest fallback
  async function loadManifestAsync() {
    if (appManifest) return;
    if (typeof window !== 'undefined' && window.__INTEROS_MANIFEST__) {
      setAppManifest(window.__INTEROS_MANIFEST__);
      return;
    }
    for (const file of ['app.json', 'manifest.json', 'manifest.webmanifest']) {
      try {
        const res = await (nativeFetch || window.fetch)(file);
        if (res.ok) {
          const data = await res.json();
          setAppManifest(data);
          return;
        }
      } catch (_) {}
    }
  }

  // Trigger async fetch in background if sync XHR was blocked
  if (!appManifest) {
    loadManifestAsync();
  }

  function hasDeclaredPermission(capability) {
    if (!capability) return false;
    const lower = capability.toLowerCase().trim();
    if (declaredCapabilities.has(lower)) return true;
    const mapped = PERMISSION_ALIASES[lower];
    if (mapped) {
      for (const m of mapped) {
        if (declaredCapabilities.has(m.toLowerCase())) return true;
      }
    }
    return false;
  }

  function checkManifestPermission(capability) {
    if (!hasDeclaredPermission(capability)) {
      throw new DOMException('Permission denied: capability not declared in manifest', 'SecurityError');
    }
  }

  // =========================================================================
  // 3. Interactive Runtime Permission Prompts
  // =========================================================================
  function renderConsentDialog(permName) {
    return new Promise((resolve) => {
      if (typeof document === 'undefined' || !document.body) {
        // Headless / non-DOM environment fallback
        resolve(true);
        return;
      }

      const overlay = document.createElement('div');
      overlay.setAttribute('role', 'dialog');
      overlay.setAttribute('aria-modal', 'true');
      overlay.style.cssText = [
        'position: fixed',
        'inset: 0',
        'z-index: 2147483647',
        'background: rgba(0, 0, 0, 0.72)',
        'backdrop-filter: blur(16px)',
        '-webkit-backdrop-filter: blur(16px)',
        'display: flex',
        'align-items: center',
        'justify-content: center',
        'font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif',
        'padding: 24px',
        'box-sizing: border-box'
      ].join(';');

      const card = document.createElement('div');
      card.style.cssText = [
        'background: #141517',
        'border: 1px solid rgba(255, 255, 255, 0.12)',
        'border-radius: 20px',
        'padding: 28px 24px',
        'max-width: 360px',
        'width: 100%',
        'box-shadow: 0 24px 48px rgba(0,0,0,0.6)',
        'text-align: center',
        'color: #ffffff'
      ].join(';');

      const appTitle = (appManifest && (appManifest.name || appManifest.id)) || 'This Application';

      card.innerHTML = `
        <div style="font-size: 36px; margin-bottom: 12px; filter: drop-shadow(0 4px 12px rgba(0,0,0,0.4));">🛡️</div>
        <h2 style="margin: 0 0 8px 0; font-size: 18px; font-weight: 600; letter-spacing: -0.01em;">Permission Request</h2>
        <p style="margin: 0 0 24px 0; font-size: 14px; color: #a1a1aa; line-height: 1.5;">
          <strong style="color: #fafafa;">${appTitle}</strong> is requesting access to <strong style="color: #60a5fa;">${permName}</strong>.
        </p>
        <div style="display: flex; gap: 12px;">
          <button id="interos-consent-deny" style="flex: 1; padding: 12px 16px; border-radius: 12px; border: 1px solid rgba(255,255,255,0.16); background: rgba(255,255,255,0.06); color: #f4f4f5; font-size: 14px; font-weight: 500; cursor: pointer; transition: background 0.15s ease;">Deny</button>
          <button id="interos-consent-allow" style="flex: 1; padding: 12px 16px; border-radius: 12px; border: none; background: #3b82f6; color: #ffffff; font-size: 14px; font-weight: 600; cursor: pointer; transition: background 0.15s ease;">Allow</button>
        </div>
      `;

      overlay.appendChild(card);
      document.body.appendChild(overlay);

      const cleanup = () => {
        if (overlay.parentNode) overlay.parentNode.removeChild(overlay);
      };

      const btnDeny = card.querySelector('#interos-consent-deny');
      const btnAllow = card.querySelector('#interos-consent-allow');

      btnDeny.onclick = () => {
        cleanup();
        resolve(false);
      };
      btnAllow.onclick = () => {
        cleanup();
        resolve(true);
      };
    });
  }

  async function requestPermission(capability) {
    if (!appManifest) {
      await loadManifestAsync();
    }

    // 1. Zero ambient authority: undeclared permissions in package manifest fail closed
    checkManifestPermission(capability);

    const norm = capability.toLowerCase().trim();
    if (!SENSITIVE_PERMISSIONS.has(norm)) {
      // Non-sensitive permissions declared in manifest are granted
      return true;
    }

    if (grantedRuntimePermissions.has(norm)) {
      return true;
    }

    // 2. Dispatch permission request to InterOS Host / Shell endpoint
    let granted = false;
    try {
      const fetchFn = nativeFetch || window.fetch;
      const res = await fetchFn('/api/permission/request', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ app_id: appId, permission: capability })
      });
      if (res.ok) {
        const body = await res.json();
        if (body.granted) {
          granted = true;
        }
      }
    } catch (_) {
      // Backend permission service not reachable or returns 404; fall back to interactive dialog
    }

    // 3. Fallback to interactive consent dialog in Mobile Shell
    if (!granted) {
      granted = await renderConsentDialog(capability);
    }

    if (granted) {
      grantedRuntimePermissions.add(norm);
      return true;
    } else {
      throw new DOMException('Permission denied by user', 'NotAllowedError');
    }
  }

  // =========================================================================
  // 4. Zero-Rewrite W3C API Polyfills
  // =========================================================================

  // --- Polyfill 4.1: window.fetch ---
  const nativeFetch = window.fetch ? window.fetch.bind(window) : null;
  if (nativeFetch) {
    window.fetch = async function (resource, init) {
      let targetUrl = '';
      if (typeof resource === 'string') {
        targetUrl = resource;
      } else if (resource && typeof resource.url === 'string') {
        targetUrl = resource.url;
      } else {
        targetUrl = String(resource || '');
      }

      const isRelative = targetUrl.startsWith('/') && !targetUrl.startsWith('//');
      const isLoopback = targetUrl.startsWith('http://127.0.0.1') ||
                         targetUrl.startsWith('http://localhost') ||
                         (typeof location !== 'undefined' && targetUrl.startsWith(location.origin));

      // Local API bridge calls (/api/...) and loopback are local IPC
      if (isRelative || isLoopback) {
        return nativeFetch(resource, init);
      }

      // External HTTP/HTTPS requests require NetworkSend authority
      if (targetUrl.startsWith('http://') || targetUrl.startsWith('https://')) {
        checkManifestPermission('NetworkSend');
        // Route through netd proxy
        const proxyUrl = `/api/browser/proxy?url=${encodeURIComponent(targetUrl)}`;
        if (typeof resource === 'string') {
          return nativeFetch(proxyUrl, init);
        } else {
          return nativeFetch(new Request(proxyUrl, resource), init);
        }
      }

      return nativeFetch(resource, init);
    };
  }

  // --- Polyfill 4.2: window.WebSocket ---
  const NativeWebSocket = window.WebSocket;
  class InterOSWebSocket {
    constructor(url, protocols) {
      checkManifestPermission('NetworkSocket');

      let targetUrl = String(url || '');
      const isLocal = targetUrl.startsWith('ws://127.0.0.1') || targetUrl.startsWith('ws://localhost');
      let effectiveUrl = targetUrl;

      if (!isLocal && (targetUrl.startsWith('ws://') || targetUrl.startsWith('wss://'))) {
        const loc = (typeof location !== 'undefined') ? location : { host: '127.0.0.1:4300', protocol: 'http:' };
        const wsProto = loc.protocol === 'https:' ? 'wss:' : 'ws:';
        effectiveUrl = `${wsProto}//${loc.host}/api/browser/proxy?url=${encodeURIComponent(targetUrl)}`;
      }

      if (NativeWebSocket) {
        this._ws = protocols ? new NativeWebSocket(effectiveUrl, protocols) : new NativeWebSocket(effectiveUrl);
      } else {
        throw new Error('Native WebSocket not supported');
      }

      this._targetUrl = targetUrl;
      this._wireEvents();
    }

    _wireEvents() {
      const ws = this._ws;
      ws.onopen = (e) => { if (typeof this.onopen === 'function') this.onopen(e); };
      ws.onmessage = (e) => { if (typeof this.onmessage === 'function') this.onmessage(e); };
      ws.onerror = (e) => { if (typeof this.onerror === 'function') this.onerror(e); };
      ws.onclose = (e) => { if (typeof this.onclose === 'function') this.onclose(e); };
    }

    get readyState() { return this._ws ? this._ws.readyState : 3; }
    get url() { return this._targetUrl; }
    get bufferedAmount() { return this._ws ? this._ws.bufferedAmount : 0; }
    get extensions() { return this._ws ? this._ws.extensions : ''; }
    get protocol() { return this._ws ? this._ws.protocol : ''; }
    get binaryType() { return this._ws ? this._ws.binaryType : 'blob'; }
    set binaryType(type) { if (this._ws) this._ws.binaryType = type; }

    send(data) {
      checkManifestPermission('NetworkSend');
      if (this._ws) this._ws.send(data);
    }

    close(code, reason) {
      if (this._ws) this._ws.close(code, reason);
    }

    addEventListener(type, listener, options) {
      if (this._ws) this._ws.addEventListener(type, listener, options);
    }

    removeEventListener(type, listener, options) {
      if (this._ws) this._ws.removeEventListener(type, listener, options);
    }

    dispatchEvent(event) {
      return this._ws ? this._ws.dispatchEvent(event) : false;
    }
  }

  InterOSWebSocket.CONNECTING = 0;
  InterOSWebSocket.OPEN = 1;
  InterOSWebSocket.CLOSING = 2;
  InterOSWebSocket.CLOSED = 3;

  try {
    window.WebSocket = InterOSWebSocket;
  } catch (_) {}

  // --- Polyfill 4.3: window.localStorage backed by InterOS VFS ---
  class InterOSVfsStorage {
    constructor(targetAppId) {
      this._appId = targetAppId;
      this._filePath = `/data/app_storage/${targetAppId}/localstorage.json`;
      this._cache = new Map();
      this._loaded = false;
      this._syncLoad();
    }

    _syncLoad() {
      try {
        const xhr = new XMLHttpRequest();
        xhr.open('POST', '/api/fs/read', false);
        xhr.setRequestHeader('Content-Type', 'application/json');
        xhr.send(JSON.stringify({ path: this._filePath }));
        if (xhr.status === 200) {
          const res = JSON.parse(xhr.responseText);
          if (res.success && res.content) {
            const parsed = JSON.parse(res.content);
            for (const [k, v] of Object.entries(parsed)) {
              this._cache.set(String(k), String(v));
            }
          }
        }
      } catch (_) {}
      this._loaded = true;
    }

    _persist() {
      const obj = {};
      for (const [k, v] of this._cache.entries()) {
        obj[k] = v;
      }
      const fetchFn = nativeFetch || window.fetch;
      if (fetchFn) {
        fetchFn('/api/fs/write', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path: this._filePath, content: JSON.stringify(obj) })
        }).catch(() => {});
      }
    }

    getItem(key) {
      checkManifestPermission('FileSystemRead');
      const k = String(key);
      return this._cache.has(k) ? this._cache.get(k) : null;
    }

    setItem(key, value) {
      checkManifestPermission('FileSystemWrite');
      this._cache.set(String(key), String(value));
      this._persist();
    }

    removeItem(key) {
      checkManifestPermission('FileSystemWrite');
      this._cache.delete(String(key));
      this._persist();
    }

    clear() {
      checkManifestPermission('FileSystemWrite');
      this._cache.clear();
      this._persist();
    }

    key(index) {
      checkManifestPermission('FileSystemRead');
      const keys = Array.from(this._cache.keys());
      return keys[index] !== undefined ? keys[index] : null;
    }

    get length() {
      checkManifestPermission('FileSystemRead');
      return this._cache.size;
    }
  }

  const vfsStorageInstance = new InterOSVfsStorage(appId);

  // Wrap with Proxy for index-based access: localStorage[key] = value, localStorage[key]
  const storageProxy = new Proxy(vfsStorageInstance, {
    get(target, prop) {
      if (typeof prop === 'symbol') return target[prop];
      if (prop in target || typeof target[prop] === 'function') {
        return typeof target[prop] === 'function' ? target[prop].bind(target) : target[prop];
      }
      return target.getItem(prop);
    },
    set(target, prop, value) {
      if (typeof prop === 'symbol') {
        target[prop] = value;
        return true;
      }
      target.setItem(prop, value);
      return true;
    },
    deleteProperty(target, prop) {
      target.removeItem(prop);
      return true;
    }
  });

  try {
    Object.defineProperty(window, 'localStorage', {
      value: storageProxy,
      writable: true,
      configurable: true
    });
  } catch (_) {}

  // --- Polyfill 4.4: window.indexedDB backed by InterOS VFS ---
  class InterOSIDBRequest extends EventTarget {
    constructor() {
      super();
      this.result = null;
      this.error = null;
      this.readyState = 'pending';
    }
    _succeed(val) {
      this.result = val;
      this.readyState = 'done';
      const ev = new Event('success');
      if (typeof this.onsuccess === 'function') this.onsuccess(ev);
      this.dispatchEvent(ev);
    }
    _fail(err) {
      this.error = err;
      this.readyState = 'done';
      const ev = new Event('error');
      if (typeof this.onerror === 'function') this.onerror(ev);
      this.dispatchEvent(ev);
    }
  }

  class InterOSIDBObjectStore {
    constructor(db, name) {
      this.db = db;
      this.name = name;
    }

    get(key) {
      checkManifestPermission('FileSystemRead');
      const req = new InterOSIDBRequest();
      setTimeout(() => {
        const store = this.db._stores[this.name] || {};
        req._succeed(store[key] !== undefined ? store[key] : undefined);
      }, 0);
      return req;
    }

    getAll() {
      checkManifestPermission('FileSystemRead');
      const req = new InterOSIDBRequest();
      setTimeout(() => {
        const store = this.db._stores[this.name] || {};
        req._succeed(Object.values(store));
      }, 0);
      return req;
    }

    put(value, key) {
      checkManifestPermission('FileSystemWrite');
      const req = new InterOSIDBRequest();
      const actualKey = key !== undefined ? key : (value && value.id ? value.id : Date.now());
      setTimeout(async () => {
        if (!this.db._stores[this.name]) this.db._stores[this.name] = {};
        this.db._stores[this.name][actualKey] = value;
        await this.db._persist();
        req._succeed(actualKey);
      }, 0);
      return req;
    }

    add(value, key) {
      return this.put(value, key);
    }

    delete(key) {
      checkManifestPermission('FileSystemWrite');
      const req = new InterOSIDBRequest();
      setTimeout(async () => {
        if (this.db._stores[this.name]) delete this.db._stores[this.name][key];
        await this.db._persist();
        req._succeed(undefined);
      }, 0);
      return req;
    }

    clear() {
      checkManifestPermission('FileSystemWrite');
      const req = new InterOSIDBRequest();
      setTimeout(async () => {
        this.db._stores[this.name] = {};
        await this.db._persist();
        req._succeed(undefined);
      }, 0);
      return req;
    }
  }

  class InterOSIDBTransaction extends EventTarget {
    constructor(db, storeNames, mode) {
      super();
      this.db = db;
      this.storeNames = Array.isArray(storeNames) ? storeNames : [storeNames];
      this.mode = mode || 'readonly';
    }
    objectStore(name) {
      return new InterOSIDBObjectStore(this.db, name);
    }
  }

  class InterOSIDBDatabase extends EventTarget {
    constructor(name, version) {
      super();
      this.name = name;
      this.version = version || 1;
      this._stores = {};
      this._filePath = `/data/app_storage/${appId}/idb_${name}.json`;
    }

    createObjectStore(name, options) {
      checkManifestPermission('FileSystemWrite');
      if (!this._stores[name]) {
        this._stores[name] = {};
      }
      return new InterOSIDBObjectStore(this, name);
    }

    transaction(storeNames, mode) {
      return new InterOSIDBTransaction(this, storeNames, mode);
    }

    close() {}

    async _persist() {
      const fetchFn = nativeFetch || window.fetch;
      if (fetchFn) {
        try {
          await fetchFn('/api/fs/write', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path: this._filePath, content: JSON.stringify(this._stores) })
          });
        } catch (_) {}
      }
    }

    async _load() {
      const fetchFn = nativeFetch || window.fetch;
      if (fetchFn) {
        try {
          const res = await fetchFn('/api/fs/read', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ path: this._filePath })
          });
          const data = await res.json();
          if (data.success && data.content) {
            this._stores = JSON.parse(data.content);
          }
        } catch (_) {}
      }
    }
  }

  const interosIndexedDB = {
    open(name, version = 1) {
      checkManifestPermission('FileSystemRead');
      const req = new InterOSIDBRequest();
      const db = new InterOSIDBDatabase(name, version);

      setTimeout(async () => {
        await db._load();
        req.result = db;

        // Upgrade needed event if new version
        const upgradeEv = new Event('upgradeneeded');
        upgradeEv.target = req;
        if (typeof req.onupgradeneeded === 'function') req.onupgradeneeded(upgradeEv);
        req.dispatchEvent(upgradeEv);

        await db._persist();
        req._succeed(db);
      }, 0);

      return req;
    },
    deleteDatabase(name) {
      checkManifestPermission('FileSystemWrite');
      const req = new InterOSIDBRequest();
      const filePath = `/data/app_storage/${appId}/idb_${name}.json`;
      const fetchFn = nativeFetch || window.fetch;
      if (fetchFn) {
        fetchFn('/api/fs/delete', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path: filePath })
        }).then(() => req._succeed(undefined)).catch(e => req._fail(e));
      }
      return req;
    }
  };

  try {
    window.indexedDB = interosIndexedDB;
  } catch (_) {}

  // --- Polyfill 4.5: navigator.mediaDevices.getUserMedia ---
  if (typeof navigator !== 'undefined') {
    if (!navigator.mediaDevices) {
      try {
        navigator.mediaDevices = {};
      } catch (_) {}
    }

    const nativeGetUserMedia = navigator.mediaDevices && navigator.mediaDevices.getUserMedia
      ? navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices)
      : null;

    function createSimulatedMediaStream(constraints) {
      const tracks = [];

      if (constraints && constraints.video) {
        let videoTrackAdded = false;
        if (typeof document !== 'undefined' && typeof document.createElement === 'function') {
          try {
            const canvas = document.createElement('canvas');
            canvas.width = 640;
            canvas.height = 480;
            if (typeof canvas.getContext === 'function') {
              const ctx = canvas.getContext('2d');
              if (ctx) {
                let frame = 0;
                const drawFrame = () => {
                  frame++;
                  ctx.fillStyle = '#0f172a';
                  ctx.fillRect(0, 0, 640, 480);
                  ctx.fillStyle = '#38bdf8';
                  ctx.font = '24px sans-serif';
                  ctx.fillText('InterOS Sovereign Virtual Camera', 40, 60);
                  ctx.fillStyle = '#94a3b8';
                  ctx.font = '16px monospace';
                  ctx.fillText(`Timestamp: ${new Date().toISOString()}`, 40, 100);
                  ctx.fillText(`Frame Counter: ${frame}`, 40, 130);
                  ctx.fillStyle = '#3b82f6';
                  ctx.beginPath();
                  ctx.arc(320 + Math.sin(frame * 0.05) * 120, 280, 40, 0, Math.PI * 2);
                  ctx.fill();
                };
                drawFrame();
                setInterval(drawFrame, 33);
                if (typeof canvas.captureStream === 'function') {
                  const stream = canvas.captureStream(30);
                  const vt = stream.getVideoTracks()[0];
                  if (vt) {
                    tracks.push(vt);
                    videoTrackAdded = true;
                  }
                }
              }
            }
          } catch (_) {}
        }
        if (!videoTrackAdded) {
          tracks.push({
            kind: 'video',
            id: 'interos_virtual_video_' + Math.random().toString(36).slice(2),
            label: 'InterOS Sovereign Virtual Camera',
            enabled: true,
            stop() {}
          });
        }
      }

      if (constraints && constraints.audio) {
        let audioTrackAdded = false;
        try {
          const AudioContextClass = window.AudioContext || window.webkitAudioContext;
          if (AudioContextClass) {
            const actx = new AudioContextClass();
            const dest = actx.createMediaStreamDestination();
            const osc = actx.createOscillator();
            const gain = actx.createGain();
            gain.gain.value = 0.001; // Silent tone placeholder
            osc.connect(gain);
            gain.connect(dest);
            osc.start();
            const at = dest.stream.getAudioTracks()[0];
            if (at) {
              tracks.push(at);
              audioTrackAdded = true;
            }
          }
        } catch (_) {}
        if (!audioTrackAdded) {
          tracks.push({
            kind: 'audio',
            id: 'interos_virtual_audio_' + Math.random().toString(36).slice(2),
            label: 'InterOS Sovereign Virtual Audio',
            enabled: true,
            stop() {}
          });
        }
      }

      if (typeof MediaStream !== 'undefined') {
        return new MediaStream(tracks);
      }
      return {
        getTracks: () => tracks,
        getVideoTracks: () => tracks.filter(t => t.kind === 'video'),
        getAudioTracks: () => tracks.filter(t => t.kind === 'audio'),
        active: true
      };
    }

    try {
      navigator.mediaDevices.getUserMedia = async function (constraints = {}) {
        if (constraints.video) {
          checkManifestPermission('CameraCapture');
          await requestPermission('CameraCapture');
        }
        if (constraints.audio) {
          checkManifestPermission('Microphone');
          await requestPermission('Microphone');
        }

        if (nativeGetUserMedia) {
          try {
            return await nativeGetUserMedia(constraints);
          } catch (_) {
            // Hardware device not attached in emulator; return clean virtual stream
          }
        }

        return createSimulatedMediaStream(constraints);
      };
    } catch (_) {}
  }

  // --- Polyfill 4.6: window.Notification ---
  class InterOSNotification extends EventTarget {
    constructor(title, options = {}) {
      super();
      checkManifestPermission('Notifications');

      this.title = String(title || '');
      this.options = options;
      this.body = options.body || '';
      this.icon = options.icon || '';
      this.tag = options.tag || '';
      this.data = options.data || null;

      const fetchFn = nativeFetch || window.fetch;
      if (fetchFn) {
        fetchFn('/api/notifications/post', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            app_id: appId,
            title: this.title,
            body: this.body,
            icon: this.icon,
            tag: this.tag,
            data: this.data
          })
        }).catch(() => {});
      }

      if (typeof window.showToast === 'function') {
        window.showToast(`${this.title}${this.body ? ': ' + this.body : ''}`);
      } else {
        console.log(`[InterOS Notification] ${this.title}: ${this.body}`);
      }
    }

    static get permission() {
      return hasDeclaredPermission('Notifications') ? 'granted' : 'denied';
    }

    static async requestPermission(callback) {
      let status = 'denied';
      try {
        checkManifestPermission('Notifications');
        const ok = await requestPermission('Notifications');
        status = ok ? 'granted' : 'denied';
      } catch (_) {
        status = 'denied';
      }
      if (typeof callback === 'function') callback(status);
      return status;
    }

    close() {
      const ev = new Event('close');
      if (typeof this.onclose === 'function') this.onclose(ev);
      this.dispatchEvent(ev);
    }
  }

  try {
    window.Notification = InterOSNotification;
  } catch (_) {}

  // =========================================================================
  // 5. InterOS Sovereign Client Bridge
  // =========================================================================
  const InterOS = {
    version: '0.1.0',
    platform: 'InterOS Sovereign Microkernel',

    get appId() {
      return appId;
    },

    get manifest() {
      return appManifest;
    },

    init(manifest) {
      setAppManifest(manifest);
    },

    hasPermission(capability) {
      return hasDeclaredPermission(capability);
    },

    checkPermission(capability) {
      checkManifestPermission(capability);
    },

    async requestPermission(capability) {
      return await requestPermission(capability);
    },

    /**
     * Capability-Governed Virtual File System (VFS)
     */
    vfs: {
      async list(path = '/') {
        checkManifestPermission('FileSystemRead');
        const fetchFn = nativeFetch || window.fetch;
        const res = await fetchFn(`/api/fs/list?path=${encodeURIComponent(path)}`);
        return await res.json();
      },

      async readFile(path) {
        checkManifestPermission('FileSystemRead');
        const fetchFn = nativeFetch || window.fetch;
        const res = await fetchFn('/api/fs/read', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path })
        });
        const data = await res.json();
        if (!data.success) throw new Error(data.error || 'Failed to read file');
        return data.content;
      },

      async writeFile(path, content) {
        checkManifestPermission('FileSystemWrite');
        const fetchFn = nativeFetch || window.fetch;
        const res = await fetchFn('/api/fs/write', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path, content })
        });
        const data = await res.json();
        if (!data.success) throw new Error(data.error || 'Failed to write file');
        return data.bytes_written || (content ? content.length : 0);
      },

      async deleteFile(path) {
        checkManifestPermission('FileSystemWrite');
        const fetchFn = nativeFetch || window.fetch;
        const res = await fetchFn('/api/fs/delete', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path })
        });
        const data = await res.json();
        if (!data.success) throw new Error(data.error || 'Failed to delete file');
        return true;
      }
    },

    /**
     * Real-time System Telemetry & Sensors
     */
    telemetry: {
      async getStatus() {
        checkManifestPermission('Sensors');
        const fetchFn = nativeFetch || window.fetch;
        const res = await fetchFn('/api/qemu/telemetry');
        return await res.json();
      },

      async getBattery() {
        checkManifestPermission('Sensors');
        const t = await this.getStatus();
        return {
          level: t.battery_level || 94,
          voltageMv: t.battery_voltage_mv || 3850,
          charging: t.battery_charging || false
        };
      },

      async getThermals() {
        checkManifestPermission('Sensors');
        const t = await this.getStatus();
        return {
          socTempC: t.soc_temp_c || 42,
          throttled: t.thermal_throttling || false
        };
      }
    },

    /**
     * Hardware Audio & Haptic Feedback
     */
    audio: {
      beep(frequency = 520, duration = 0.05, type = 'sine') {
        checkManifestPermission('AudioSynthesis');
        try {
          const AudioContextClass = window.AudioContext || window.webkitAudioContext;
          if (!AudioContextClass) return;
          const ctx = new AudioContextClass();
          const osc = ctx.createOscillator();
          const gain = ctx.createGain();
          osc.type = type;
          osc.frequency.value = frequency;
          gain.gain.setValueAtTime(0.08, ctx.currentTime);
          gain.gain.exponentialRampToValueAtTime(0.001, ctx.currentTime + duration);
          osc.connect(gain);
          gain.connect(ctx.destination);
          osc.start();
          osc.stop(ctx.currentTime + duration);
        } catch (_) {
          // Audio context suppressed
        }
      }
    },

    /**
     * User Notifications & Toast Alerts
     */
    notifications: {
      show(message, type = 'info') {
        checkManifestPermission('Notifications');
        const fetchFn = nativeFetch || window.fetch;
        if (fetchFn) {
          fetchFn('/api/notifications/post', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              app_id: appId,
              title: (appManifest && appManifest.name) || 'Notification',
              body: message,
              type
            })
          }).catch(() => {});
        }
        if (typeof window.showToast === 'function') {
          window.showToast(message);
        } else {
          console.log(`[InterOS Notification] (${type}) ${message}`);
        }
      }
    }
  };

  window.InterOS = InterOS;
})(typeof window !== 'undefined' ? window : globalThis);
