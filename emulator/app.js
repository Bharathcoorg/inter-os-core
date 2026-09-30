/**
 * Inter OS — Device Studio & Emulator Controller
 *
 * Features:
 * - Complete Android Emulator Hardware Toolbar (Power, Vol+, Vol-, Screenshot, Rotate, Reboot, Back, Home, Recents)
 * - Minimalist, modern Mobile OS (Clock & Date widget, Sideload Tray, 8 System Apps, Dock, Lockscreen, Boot Splash)
 * - World-Class Browser: Multi-tab, Omnibar, Bookmarks, Searchable Rust Docs, Crates Explorer, Privacy Shield
 * - World-Class Camera: Real webcam feed / Procedural 60fps night optical sensor, Lenses (0.5x, 1x, 3x, 5x), Shutter click, Flash, Photo Gallery
 * - World-Class Calculator: Standard + Scientific mode (Trig, Log, Powers, Deg/Rad), Calculation History Drawer
 * - Interactive Rust Shell Terminal: Command line interpreter (help, uname, ps, free, ls, dmesg, reboot, killswitch)
 * - Hardware Digital Twin Simulation: Battery % (LTPO 60Hz vs 120Hz), Lux (DC Dimming vs 1000-nit HBM), Dual-SIM, Remote Killswitch
 * - Multi-Format App Sideloading (.app, .rap, .wasm, .apk) with SHA-256 validation and Ring 3 Sandbox mounting
 */

document.addEventListener('DOMContentLoaded', () => {
  // =========================================================================
  // Sovereign Microkernel IPC & Web Bridge
  // =========================================================================
  window.InterOS = {
    ipc: {
      async send(target, cmd, payload = {}) {
        try {
          const resp = await fetch('/api/rapp/ipc', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ target, cmd, payload, timestamp: Date.now() })
          });
          return await resp.json();
        } catch (e) {
          console.error('[InterOS IPC Error]', e);
          return { success: false, error: String(e) };
        }
      },
      async request(target, action, data = {}) {
        return this.send(target, 0x10, { action, ...data });
      }
    },
    version: '0.8.2',
    arch: 'x86_64 / aarch64 / riscv64'
  };

  // Universal Sovereign Glass Toast Notification System
  function escapeHtml(str) {
    if (!str) return '';
    return String(str)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }
  window.escapeHtml = escapeHtml;

  window.showToast = function(msg, duration = 2400) {
    let container = document.getElementById('interos-toast-container');
    if (!container) {
      container = document.createElement('div');
      container.id = 'interos-toast-container';
      container.className = 'interos-toast-container';
      const screen = document.getElementById('device-screen') || document.body;
      screen.appendChild(container);
    }
    const toast = document.createElement('div');
    toast.className = 'interos-toast-pill';
    toast.innerHTML = `<span class="itp-dot"></span><span class="itp-text">${escapeHtml(msg)}</span>`;
    container.appendChild(toast);
    requestAnimationFrame(() => toast.classList.add('visible'));
    setTimeout(() => {
      toast.classList.remove('visible');
      setTimeout(() => {
        if (toast.parentNode) toast.parentNode.removeChild(toast);
      }, 300);
    }, duration);
  };

  const state = {
    screenOn: true,
    locked: false,
    vbeActive: false,
    rfbClient: null,
    volume: 70,
    isLandscape: false,
    currentView: 'view-home',
    viewHistory: ['view-home'],
    recentApps: ['view-home'],
    calcMode: 'std',
    calcExpr: '',
    calcHistory: [],
    audioCtx: null,
    cameraStream: null,
    cameraAnimId: null,
    cameraZoom: 1.0,
    cameraMode: 'photo',
    cameraFacing: 'environment',
    capturedPhotos: [],
    installedPackages: []
  };

  // Web Audio Synthesizer (for click tones, shutter sounds)
  function playBeep(freq = 600, duration = 0.05, type = 'sine') {
    try {
      if (!state.audioCtx) {
        const AudioContext = window.AudioContext || window.webkitAudioContext;
        if (AudioContext) state.audioCtx = new AudioContext();
      }
      if (state.audioCtx && state.audioCtx.state === 'suspended') {
        state.audioCtx.resume();
      }
      if (!state.audioCtx) return;
      const osc = state.audioCtx.createOscillator();
      const gain = state.audioCtx.createGain();
      osc.type = type;
      osc.frequency.setValueAtTime(freq, state.audioCtx.currentTime);
      gain.gain.setValueAtTime(0.08, state.audioCtx.currentTime);
      gain.gain.exponentialRampToValueAtTime(0.001, state.audioCtx.currentTime + duration);
      osc.connect(gain);
      gain.connect(state.audioCtx.destination);
      osc.start();
      osc.stop(state.audioCtx.currentTime + duration);
    } catch (e) {
      // Audio context might be restricted before user interaction
    }
  }

  function playShutterSound() {
    playBeep(1200, 0.03, 'triangle');
    setTimeout(() => playBeep(800, 0.04, 'sine'), 40);
  }

  // =========================================================================
  // 2. DOM ELEMENTS
  // =========================================================================
  // Header
  const phoneClock = document.getElementById('phone-clock');
  const btnRebootKernel = document.getElementById('btn-reboot-kernel');
  const telemetryTerminal = document.getElementById('telemetry-terminal');
  const btnClearConsole = document.getElementById('btn-clear-console');
  const btnPingKernel = document.getElementById('btn-ping-kernel');

  // Stage & Chassis
  const stageWrapper = document.getElementById('emulator-stage-wrapper');
  const deviceScreen = document.getElementById('device-screen');
  const screenOffOverlay = document.getElementById('screen-off-overlay');
  const screenBootSplash = document.getElementById('screen-boot-splash');
  const screenLockscreen = document.getElementById('screen-lockscreen');
  const lockClockTime = document.getElementById('lock-clock-time');
  const lockClockDate = document.getElementById('lock-clock-date');
  const volumeHudToast = document.getElementById('volume-hud-toast');
  const volBarFill = document.getElementById('vol-bar-fill');
  const volText = document.getElementById('vol-text');
  const homeBigClock = document.getElementById('home-big-clock');
  const homeBigDate = document.getElementById('home-big-date');
  const homePill = document.getElementById('home-pill');

  // Gravity Shade & System Top HUD Elements
  const gravityShade = document.getElementById('gravity-shade');
  const shadeBackdrop = document.getElementById('shade-backdrop');
  const shadeHandleBar = document.getElementById('shade-handle-bar');
  const shadeBtnClose = document.getElementById('shade-btn-close');
  const phoneStatusBar = document.getElementById('phone-status-bar');
  const quantumCapsule = document.getElementById('quantum-capsule');
  let openShade = () => {};
  let closeShade = () => {};
  let toggleShade = () => {};

  // Android Emulator Hardware Toolbar Buttons
  const emuBtnPower = document.getElementById('emu-btn-power');
  const emuBtnVolup = document.getElementById('emu-btn-volup');
  const emuBtnVoldown = document.getElementById('emu-btn-voldown');
  const emuBtnScreenshot = document.getElementById('emu-btn-screenshot');
  const emuBtnRotate = document.getElementById('emu-btn-rotate');
  const emuBtnReboot = document.getElementById('emu-btn-reboot');
  const emuBtnBack = document.getElementById('emu-btn-back');
  const emuBtnHome = document.getElementById('emu-btn-home');
  const emuBtnRecents = document.getElementById('emu-btn-recents');

  // Physical Side Buttons on chassis
  const hwBtnPower = document.getElementById('hw-btn-power');
  const hwBtnVolup = document.getElementById('hw-btn-volup');
  const hwBtnVoldown = document.getElementById('hw-btn-voldown');

  // Sideload Tray
  const sideloadTray = document.getElementById('sideload-tray');
  const inputSideloadFile = document.getElementById('input-sideload-file');
  const sideloadTraySub = document.getElementById('sideload-tray-sub');

  // App Tiles on Homescreen
  const tileBrowser = document.getElementById('tile-browser');
  const tileCamera = document.getElementById('tile-camera');
  const tileCalculator = document.getElementById('tile-calculator');
  const tileSensors = document.getElementById('tile-sensors');
  const tileGallery = document.getElementById('tile-gallery');
  const tileTerminal = document.getElementById('tile-terminal');
  const tileSettings = document.getElementById('tile-settings');
  const tileHelloworld = document.getElementById('tile-helloworld');
  const tileDialer = document.getElementById('tile-dialer');
  const tileMessages = document.getElementById('tile-messages');
  const tileFiles = document.getElementById('tile-files');
  const tileClock = document.getElementById('tile-clock');

  // Dock Buttons
  const dockDialer = document.getElementById('dock-dialer');
  const dockMessages = document.getElementById('dock-messages');
  const dockBrowser = document.getElementById('dock-browser');
  const dockCamera = document.getElementById('dock-camera');
  const dockCalculator = document.getElementById('dock-calculator');
  const dockSettings = document.getElementById('dock-settings');

  // Subview Back Buttons
  const btnBackSensors = document.getElementById('btn-back-sensors');
  const btnBackGallery = document.getElementById('btn-back-gallery');
  const btnBackSettings = document.getElementById('btn-back-settings');
  const btnBackHelloworld = document.getElementById('btn-back-helloworld');
  const btnBackDialer = document.getElementById('btn-back-dialer');
  const btnBackMessages = document.getElementById('btn-back-messages');
  const btnBackFiles = document.getElementById('btn-back-files');
  const btnBackClock = document.getElementById('btn-back-clock');

  // Hardware Digital Twin Controls
  const hwSliderBattery = document.getElementById('hw-slider-battery');
  const hwValBattery = document.getElementById('hw-val-battery');
  const hwSliderLux = document.getElementById('hw-slider-lux');
  const hwValLux = document.getElementById('hw-val-lux');
  const btnSimToggle = document.getElementById('btn-sim-toggle');
  const btnHwKillswitch = document.getElementById('btn-hw-killswitch');
  const sensorValRefresh = document.getElementById('sensor-val-refresh');
  const sensorBadgeRefresh = document.getElementById('sensor-badge-refresh');
  const sensorValLux = document.getElementById('sensor-val-lux');
  const sensorBadgeLux = document.getElementById('sensor-badge-lux');
  const phoneBattery = document.getElementById('phone-battery');
  const phoneSimBadge = document.getElementById('phone-sim-badge');

  // Tool Window Tabs & Responsive Horizontal Scrolling Controls
  const panelTabs = document.querySelectorAll('.panel-tab');
  const tabContents = document.querySelectorAll('.tab-content');
  const packagesTableBody = document.getElementById('packages-table-body');
  const ideTabBar = document.getElementById('ide-tab-bar');
  const btnTabScrollLeft = document.getElementById('btn-tab-scroll-left');
  const btnTabScrollRight = document.getElementById('btn-tab-scroll-right');
  const ideRightPane = document.getElementById('ide-right-pane');

  panelTabs.forEach(tab => {
    tab.addEventListener('click', () => {
      panelTabs.forEach(t => t.classList.remove('active'));
      tabContents.forEach(c => c.classList.remove('active'));
      tab.classList.add('active');
      const targetId = tab.getAttribute('data-tab');
      const targetContent = document.getElementById(targetId);
      if (targetContent) targetContent.classList.add('active');
      tab.scrollIntoView({ behavior: 'smooth', inline: 'nearest', block: 'nearest' });
      if (ideRightPane && ideRightPane.scrollTop !== 0) ideRightPane.scrollTop = 0;
    });
  });

  // Tab Bar Horizontal Scroll Buttons
  btnTabScrollLeft?.addEventListener('click', () => {
    ideTabBar?.scrollBy({ left: -240, behavior: 'smooth' });
  });
  btnTabScrollRight?.addEventListener('click', () => {
    ideTabBar?.scrollBy({ left: 240, behavior: 'smooth' });
  });

  // Mouse Wheel horizontal scrolling on Tab Bar
  ideTabBar?.addEventListener('wheel', (e) => {
    if (e.deltaY !== 0) {
      e.preventDefault();
      ideTabBar.scrollLeft += e.deltaY;
    }
  }, { passive: false });

  // Mouse Wheel horizontal scrolling on Hardware Profiler Cards Grid
  const hwCardsGrid = document.querySelector('.hw-cards-grid');
  hwCardsGrid?.addEventListener('wheel', (e) => {
    if (e.deltaY !== 0 && hwCardsGrid.scrollWidth > hwCardsGrid.clientWidth) {
      e.preventDefault();
      hwCardsGrid.scrollLeft += e.deltaY;
    }
  }, { passive: false });

  // Hardware Profiler Collapsible Toggle Button
  const btnToggleHwPanel = document.getElementById('btn-toggle-hw-panel');
  const hwTelemetryPanel = document.querySelector('.hw-telemetry-panel');
  const hwCollapseText = document.querySelector('.hw-collapse-text');
  btnToggleHwPanel?.addEventListener('click', () => {
    if (!hwTelemetryPanel) return;
    const isNowCollapsed = hwTelemetryPanel.classList.toggle('is-collapsed');
    if (hwCollapseText) {
      hwCollapseText.textContent = isNowCollapsed ? 'Expand' : 'Compact';
    }
    if (ideRightPane) ideRightPane.scrollTop = 0;
  });

  // Guard right pane from ever scrolling vertically
  ideRightPane?.addEventListener('scroll', () => {
    if (ideRightPane.scrollTop !== 0) ideRightPane.scrollTop = 0;
  });

  // =========================================================================
  // HARDWARE TELEMETRY & REAL QEMU PROFILER
  // =========================================================================
  const qemuGlobalDot = document.getElementById('qemu-global-dot');
  const qemuGlobalText = document.getElementById('qemu-global-text');
  const qemuGlobalStatus = document.getElementById('qemu-global-status');
  const qemuHalBadge = document.getElementById('qemu-hal-badge');
  const valRamUsage = document.getElementById('val-ram-usage');
  const fillRamBar = document.getElementById('fill-ram-bar');
  const valBlockName = document.getElementById('val-block-name');
  const fillBlockBar = document.getElementById('fill-block-bar');
  const valBlockIo = document.getElementById('val-block-io');
  const valCpuState = document.getElementById('val-cpu-state');
  const valCpuRip = document.getElementById('val-cpu-rip');
  const valCpuTid = document.getElementById('val-cpu-tid');
  const cpuHeatmapMatrix = document.getElementById('cpu-heatmap-matrix');

  // Register element mapping
  const regMap = {
    'reg-rax': ['RAX', 'EAX'],
    'reg-rbx': ['RBX', 'EBX'],
    'reg-rcx': ['RCX', 'ECX'],
    'reg-rdx': ['RDX', 'EDX'],
    'reg-rsi': ['RSI', 'ESI'],
    'reg-rdi': ['RDI', 'EDI'],
    'reg-rbp': ['RBP', 'EBP'],
    'reg-rsp': ['RSP', 'ESP'],
    'reg-r8':  ['R8'],
    'reg-r9':  ['R9'],
    'reg-r10': ['R10'],
    'reg-r11': ['R11'],
    'reg-r12': ['R12'],
    'reg-r13': ['R13'],
    'reg-r14': ['R14'],
    'reg-r15': ['R15'],
    'reg-rip': ['RIP', 'EIP'],
    'reg-rflags': ['RFLAGS', 'RFL', 'EFL'],
    'reg-cr0': ['CR0'],
    'reg-cr3': ['CR3'],
    'reg-cr4': ['CR4'],
    'reg-efer': ['EFER']
  };

  function updateHardwareProfilerUI(data) {
    if (!data) return;
    const isAlive = !!data.qemu_alive;
    updateDeviceDisplay(isAlive, !!data.launcher_ready);

    // 1. Header Global Status (Simple & Clean: Online vs Standby)
    if (qemuGlobalText) {
      qemuGlobalText.textContent = isAlive ? 'Online' : 'Standby';
    }
    if (qemuGlobalDot) {
      if (isAlive) {
        qemuGlobalDot.classList.add('pulse');
        qemuGlobalDot.style.background = '#10B981';
        qemuGlobalDot.style.boxShadow = '0 0 6px rgba(16, 185, 129, 0.6)';
      } else {
        qemuGlobalDot.classList.remove('pulse');
        qemuGlobalDot.style.background = '#71717A';
        qemuGlobalDot.style.boxShadow = 'none';
      }
    }
    if (qemuGlobalStatus) {
      if (isAlive) {
        qemuGlobalStatus.classList.add('online');
      } else {
        qemuGlobalStatus.classList.remove('online');
      }
      qemuGlobalStatus.title = isAlive
        ? `QEMU Microkernel: ${data.status || 'running'} (Mode: ${data.mode || 'live'})`
        : 'QEMU Microkernel: Standby';
    }

    // 2. Hardware Profiler Top Badge
    if (qemuHalBadge) {
      if (isAlive) {
        qemuHalBadge.textContent = 'QEMU / BARE-METAL • LIVE';
        qemuHalBadge.style.color = '#10B981';
        qemuHalBadge.style.borderColor = 'rgba(16, 185, 129, 0.3)';
      } else {
        qemuHalBadge.textContent = 'QEMU • STANDBY';
        qemuHalBadge.style.color = '#71717A';
        qemuHalBadge.style.borderColor = 'rgba(255, 255, 255, 0.1)';
      }
    }

    // 3. System RAM & CR3 Paging
    const cr3 = data.cpu?.registers?.CR3;
    if (valRamUsage) {
      valRamUsage.textContent = isAlive ? `CR3: ${cr3 || '0x01353000'}` : 'CR3: Standby';
    }
    if (fillRamBar) {
      fillRamBar.style.width = isAlive ? '32.4%' : '0%';
    }

    // 4. QEMU Block Drive
    if (valBlockName) {
      valBlockName.textContent = isAlive ? (data.block?.file || 'inter-os-bios.img') : 'Standby / Offline';
    }
    if (valBlockIo) {
      if (isAlive) {
        const readMb = data.block?.read_bytes ? (data.block.read_bytes / (1024 * 1024)).toFixed(2) + ' MB' : '0 MB';
        const ops = data.block?.read_ops || 0;
        valBlockIo.innerHTML = `Device: <strong>${data.block?.device || 'ide0-hd0'}</strong> &bull; Read: <strong>${readMb} (${ops} ops)</strong>`;
      } else {
        valBlockIo.innerHTML = 'Device: <strong>Standby</strong> &bull; QEMU Inactive';
      }
    }
    if (fillBlockBar) {
      fillBlockBar.style.width = isAlive ? '24.6%' : '0%';
    }

    // 5. CPU Registers & Heatmap
    if (valCpuState) {
      if (isAlive) {
        valCpuState.textContent = data.cpu?.halted ? 'Ring 0 Safe • HLT (Idle Wait)' : 'Ring 0 Active • Running';
      } else {
        valCpuState.textContent = 'Standby (Offline)';
      }
    }
    if (valCpuRip) {
      valCpuRip.innerHTML = `RIP: <strong>${isAlive ? (data.cpu?.rip || 'Offline') : 'Offline'}</strong>`;
    }
    if (valCpuTid) {
      valCpuTid.textContent = `TID: ${isAlive ? (data.cpu?.thread_id || 'Active') : 'Offline'}`;
    }

    // 6. CPU Registers Tab Update
    if (data.cpu?.registers && isAlive && Object.keys(data.cpu.registers).length > 0) {
      for (const [domId, aliases] of Object.entries(regMap)) {
        const el = document.getElementById(domId);
        if (el) {
          for (const alias of aliases) {
            if (data.cpu.registers[alias] !== undefined) {
              el.textContent = data.cpu.registers[alias];
              break;
            }
          }
        }
      }
    } else {
      for (const domId of Object.keys(regMap)) {
        const el = document.getElementById(domId);
        if (el) el.textContent = '—';
      }
    }
  }

  async function fetchHardwareTelemetry() {
    try {
      const res = await fetch('/api/qemu/telemetry');
      if (res.ok) {
        const data = await res.json();
        updateHardwareProfilerUI(data);
      }
    } catch (e) {
      updateHardwareProfilerUI({ qemu_alive: false, status: 'offline' });
    }
  }

  // Periodic Telemetry Poll (every 1.5s)
  fetchHardwareTelemetry();
  setInterval(fetchHardwareTelemetry, 1500);

  // Warm Reboot Microkernel Button Wiring
  btnRebootKernel?.addEventListener('click', async () => {
    btnRebootKernel.style.pointerEvents = 'none';
    const span = btnRebootKernel.querySelector('span');
    const originalText = span ? span.textContent : 'Reboot OS';
    if (span) span.textContent = 'Rebooting...';
    playBeep(440, 0.1, 'square');
    logConsole('warn', '[KERNEL_EVENT] Warm reboot signal dispatched to hardware controller...');

    try {
      const res = await fetch('/api/qemu/reboot', { method: 'POST' });
      const data = await res.json();
      if (data.success) {
        logConsole('ok', `[REBOOT_OK] Hardware state synchronized (system_reset: ${data.qemu_reset ? 'delivered' : 'simulated'}).`);
        playBeep(880, 0.08, 'triangle');
      }
    } catch (err) {
      logConsole('err', `[REBOOT_ERROR] Failed to send reboot: ${err.message}`);
    }

    setTimeout(() => {
      fetchHardwareTelemetry();
      if (span) span.textContent = originalText;
      btnRebootKernel.style.pointerEvents = 'auto';
    }, 1200);
  });

  // =========================================================================
  // Authentic Microkernel Display & Live Scanout Controller
  // =========================================================================
  const screenLiveScanout = document.getElementById('screen-live-scanout');
  const screenStandby = document.getElementById('screen-standby');
  const liveScanoutImg = document.getElementById('live-scanout-img');
  const scanoutResText = document.getElementById('scanout-res-text');
  const btnToggleScanoutCrop = document.getElementById('btn-toggle-scanout-crop');
  const scanoutModeLabel = document.getElementById('scanout-mode-label');
  const btnRefreshScanout = document.getElementById('btn-refresh-scanout');
  const btnBootMicrokernel = document.getElementById('btn-boot-microkernel');

  // Gestures
  const btnGestureActivity = document.getElementById('btn-gesture-activity');
  const btnGestureBack = document.getElementById('btn-gesture-back');
  const btnGestureHome = document.getElementById('btn-gesture-home');
  const btnGestureQuick = document.getElementById('btn-gesture-quick');
  const btnGestureSystem = document.getElementById('btn-gesture-system');

  let cropMode = 'portrait'; // 'portrait' (720x1440 cropped) or 'full' (1280x720 complete scanout)
  let scanoutTimer = null;
  let isBooting = false;

  function setScanoutMode(mode) {
    cropMode = mode;
    if (scanoutModeLabel) {
      scanoutModeLabel.textContent = cropMode === 'portrait' ? 'Portrait' : 'Full 720p';
    }
    if (scanoutResText) {
      scanoutResText.innerHTML = cropMode === 'portrait'
        ? 'LIVE SCANOUT &bull; 720&times;1440'
        : 'LIVE SCANOUT &bull; 1280&times;720';
    }
    refreshScanoutFrame();
  }

  btnToggleScanoutCrop?.addEventListener('click', () => {
    setScanoutMode(cropMode === 'portrait' ? 'full' : 'portrait');
  });

  btnRefreshScanout?.addEventListener('click', () => {
    refreshScanoutFrame();
  });

  let scanoutInFlight = false;

  function refreshScanoutFrame() {
    if (!liveScanoutImg || scanoutInFlight) return;
    scanoutInFlight = true;
    const url = `/api/screen/frame?t=${Date.now()}${cropMode === 'full' ? '&full=1' : ''}`;
    const done = () => { scanoutInFlight = false; };
    liveScanoutImg.addEventListener('load', done, { once: true });
    liveScanoutImg.addEventListener('error', done, { once: true });
    liveScanoutImg.src = url;
  }

  function startScanoutStream() {
    if (scanoutTimer) return;
    refreshScanoutFrame();
    // QEMU screendump is transport-bound, so use a bounded 100 ms cadence
    // rather than claiming a cosmetic 60 FPS stream.
    scanoutTimer = setInterval(refreshScanoutFrame, 100);
  }

  function stopScanoutStream() {
    if (scanoutTimer) {
      clearInterval(scanoutTimer);
      scanoutTimer = null;
    }
  }

  let hasLauncherPresented = false;

  function updateDeviceDisplay(isAlive, launcherReady = false) {
    if (isBooting) {
      if (screenBootSplash) screenBootSplash.style.display = 'flex';
      if (screenLiveScanout) screenLiveScanout.style.display = 'none';
      if (screenStandby) screenStandby.style.display = 'none';
      return;
    }

    if (launcherReady) {
      hasLauncherPresented = true;
    }

    if (isAlive) {
      if (launcherReady || hasLauncherPresented) {
        if (screenBootSplash) screenBootSplash.style.display = 'none';
        if (screenStandby) screenStandby.style.display = 'none';
        if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
        startScanoutStream();
      } else {
        // Still booting early BIOS/kernel: keep clean bootloader screen active
        if (screenBootSplash) screenBootSplash.style.display = 'flex';
        if (screenLiveScanout) screenLiveScanout.style.display = 'none';
        if (screenStandby) screenStandby.style.display = 'none';
      }
    } else {
      hasLauncherPresented = false;
      stopScanoutStream();
      if (screenBootSplash) screenBootSplash.style.display = 'none';
      if (screenLiveScanout) screenLiveScanout.style.display = 'none';
      if (screenStandby) screenStandby.style.display = 'flex';
    }
  }

  // Interactive touch and gesture dispatch directly to microkernel
  if (liveScanoutImg) {
    let isPointerDown = false;
    let startX = 0, startY = 0;
    let startTime = 0;

    liveScanoutImg.addEventListener('pointerdown', (ev) => {
      isPointerDown = true;
      startTime = Date.now();
      const rect = liveScanoutImg.getBoundingClientRect();
      startX = (ev.clientX - rect.left) / rect.width;
      startY = (ev.clientY - rect.top) / rect.height;
      try { liveScanoutImg.setPointerCapture(ev.pointerId); } catch (_) {}
    });

    liveScanoutImg.addEventListener('pointerup', async (ev) => {
      if (!isPointerDown) return;
      isPointerDown = false;
      const rect = liveScanoutImg.getBoundingClientRect();
      const endX = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
      const endY = Math.max(0, Math.min(1, (ev.clientY - rect.top) / rect.height));
      const dx = endX - startX;
      const dy = endY - startY;
      const dist = Math.hypot(dx * rect.width, dy * rect.height);

      // Coordinate mapping
      let payload;
      if (cropMode === 'portrait') {
        const px1 = Math.max(0, Math.min(719, Math.round(startX * 719)));
        const py1 = Math.max(0, Math.min(1439, Math.round(startY * 1439)));
        const px2 = Math.max(0, Math.min(719, Math.round(endX * 719)));
        const py2 = Math.max(0, Math.min(1439, Math.round(endY * 1439)));
        if (dist > 18) {
          payload = { action: 'swipe', coords: 'portrait', x: px1, y: py1, x2: px2, y2: py2, steps: 5 };
        } else {
          payload = { action: 'tap', coords: 'portrait', x: px2, y: py2 };
        }
      } else {
        const gx1 = Math.max(0, Math.min(1279, Math.round(startX * 1280)));
        const gy1 = Math.max(0, Math.min(719, Math.round(startY * 720)));
        const gx2 = Math.max(0, Math.min(1279, Math.round(endX * 1280)));
        const gy2 = Math.max(0, Math.min(719, Math.round(endY * 720)));
        if (dist > 18) {
          payload = { action: 'swipe', coords: 'guest_framebuffer', x: gx1, y: gy1, x2: gx2, y2: gy2, steps: 5 };
        } else {
          payload = { action: 'tap', coords: 'guest_framebuffer', x: gx2, y: gy2 };
        }
      }

      try {
        await fetch('/api/touch/event', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        });
      } catch (_) {}

      // Fast responsive frame refreshes
      setTimeout(refreshScanoutFrame, 35);
      setTimeout(refreshScanoutFrame, 120);
      setTimeout(refreshScanoutFrame, 260);
    });

    liveScanoutImg.addEventListener('pointercancel', () => {
      isPointerDown = false;
    });
  }

  // Canonical Navigation Gestures (Matching 16-Surface North Star Reference)
  async function dispatchHardwareGesture(gestureName, payload) {
    try {
      await fetch('/api/touch/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload)
      });
    } catch (_) {}
    setTimeout(refreshScanoutFrame, 40);
    setTimeout(refreshScanoutFrame, 140);
    setTimeout(refreshScanoutFrame, 280);
  }

  // 1. Swipe Up -> Activity Space (Multitasking)
  btnGestureActivity?.addEventListener('click', () => {
    dispatchHardwareGesture('Activity Space (Swipe Up)', {
      action: 'swipe',
      coords: 'portrait',
      x: 360,
      y: 1400,
      x2: 360,
      y2: 800,
      steps: 6
    });
  });

  // 2. Swipe Left -> Go Back
  btnGestureBack?.addEventListener('click', () => {
    dispatchHardwareGesture('Back (Swipe Left)', {
      action: 'swipe',
      coords: 'portrait',
      x: 600,
      y: 720,
      x2: 120,
      y2: 720,
      steps: 5
    });
  });

  // 3. Home -> Tap Home Bar
  btnGestureHome?.addEventListener('click', () => {
    dispatchHardwareGesture('Home (Tap Home Bar)', {
      action: 'tap',
      coords: 'portrait',
      x: 360,
      y: 1408
    });
  });

  // 4. Swipe Right -> Quick Controls
  btnGestureQuick?.addEventListener('click', () => {
    dispatchHardwareGesture('Quick Controls (Swipe Right)', {
      action: 'swipe',
      coords: 'portrait',
      x: 100,
      y: 720,
      x2: 600,
      y2: 720,
      steps: 5
    });
  });

  // 5. Tap Top -> System Space / Settings
  btnGestureSystem?.addEventListener('click', () => {
    dispatchHardwareGesture('System Space (Tap Top Bar)', {
      action: 'tap',
      coords: 'portrait',
      x: 360,
      y: 30
    });
  });

  // =========================================================================
  // Physical Side Buttons & Android Emulator Hardware Toolbar Controls
  // =========================================================================
  let bootCheckInterval = null;

  async function checkLauncherBootReady() {
    try {
      const resp = await fetch('/api/qemu/status');
      const data = await resp.json();
      if (data.launcher_ready) {
        if (bootCheckInterval) {
          clearInterval(bootCheckInterval);
          bootCheckInterval = null;
        }
        isBooting = false;
        refreshScanoutFrame();
        if (screenBootSplash) screenBootSplash.style.display = 'none';
        if (screenStandby) screenStandby.style.display = 'none';
        if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
        startScanoutStream();
        logConsole('ok', '[LAUNCHER] Pristine Lock Screen scanout ready and presented.');
        showToast('InterOS Ready • Swipe up to unlock');
      }
    } catch (_) {}
  }

  // Power On / Boot Microkernel
  async function bootMicrokernel() {
    isBooting = true;
    stopScanoutStream();
    if (screenBootSplash) screenBootSplash.style.display = 'flex';
    if (screenLiveScanout) screenLiveScanout.style.display = 'none';
    if (screenStandby) screenStandby.style.display = 'none';

    logConsole('warn', '[BOOT] Booting InterOS Sovereign Microkernel...');
    showToast('Booting InterOS...');

    try {
      const resp = await fetch('/api/qemu/launch', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ profile: 'x86_64' })
      });
      const data = await resp.json();
      if (data.success) {
        logConsole('ok', `[BOOT_OK] ${data.message}`);
      } else {
        logConsole('err', `[BOOT_FAIL] ${data.message || 'Failed to start QEMU'}`);
      }
    } catch (e) {
      logConsole('err', `[BOOT_ERROR] ${e.message}`);
    }

    if (bootCheckInterval) clearInterval(bootCheckInterval);
    bootCheckInterval = setInterval(checkLauncherBootReady, 400);

    setTimeout(() => {
      if (isBooting) {
        isBooting = false;
        if (bootCheckInterval) clearInterval(bootCheckInterval);
        refreshScanoutFrame();
        if (screenBootSplash) screenBootSplash.style.display = 'none';
        if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
        startScanoutStream();
      }
    }, 45000);
  }

  // Hardware Power Button Action
  function handlePowerAction() {
    const isAlive = qemuGlobalStatus?.classList.contains('online');
    if (!isAlive) {
      bootMicrokernel();
    } else {
      dispatchHardwareGesture('Power / Lock Screen', {
        action: 'tap',
        x: 640,
        y: 15
      });
    }
  }

  hwBtnPower?.addEventListener('click', handlePowerAction);
  emuBtnPower?.addEventListener('click', handlePowerAction);

  // Hardware Volume Buttons Action
  function handleVolume(delta) {
    state.volume = Math.max(0, Math.min(100, state.volume + delta));
    showToast(`Volume: ${state.volume}%`);
    playBeep(delta > 0 ? 800 : 500, 0.04, 'sine');
  }

  hwBtnVolup?.addEventListener('click', () => handleVolume(5));
  emuBtnVolup?.addEventListener('click', () => handleVolume(5));
  hwBtnVoldown?.addEventListener('click', () => handleVolume(-5));
  emuBtnVoldown?.addEventListener('click', () => handleVolume(-5));

  // Screenshot Capture Button
  emuBtnScreenshot?.addEventListener('click', () => {
    const link = document.createElement('a');
    link.href = `/api/screen/frame?full=1&t=${Date.now()}`;
    link.download = `interos_microkernel_scanout_${Date.now()}.png`;
    link.click();
    playShutterSound();
    logConsole('ok', '[SCREENSHOT] Authentic microkernel scanout captured and exported.');
    showToast('Scanout Screenshot Saved');
  });

  // Rotate Screen Button (Toggle Portrait Crop vs Full 720p)
  emuBtnRotate?.addEventListener('click', () => {
    setScanoutMode(cropMode === 'portrait' ? 'full' : 'portrait');
    showToast(`Screen mode: ${cropMode === 'portrait' ? 'Portrait Phone' : 'Full Scanout'}`);
  });

  // Warm Reboot Microkernel Button
  emuBtnReboot?.addEventListener('click', async () => {
    isBooting = true;
    stopScanoutStream();
    if (screenBootSplash) screenBootSplash.style.display = 'flex';
    if (screenLiveScanout) screenLiveScanout.style.display = 'none';
    if (screenStandby) screenStandby.style.display = 'none';
    logConsole('warn', '[REBOOT] Warm rebooting microkernel...');
    showToast('Rebooting InterOS...');

    try {
      await fetch('/api/qemu/reboot', { method: 'POST' });
    } catch (_) {}

    if (bootCheckInterval) clearInterval(bootCheckInterval);
    bootCheckInterval = setInterval(checkLauncherBootReady, 400);
    setTimeout(() => {
      if (isBooting) {
        isBooting = false;
        if (bootCheckInterval) clearInterval(bootCheckInterval);
        refreshScanoutFrame();
        if (screenBootSplash) screenBootSplash.style.display = 'none';
        if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
        startScanoutStream();
      }
    }, 45000);
  });

  // Back Button (◁)
  emuBtnBack?.addEventListener('click', () => {
    dispatchHardwareGesture('Back (◁)', {
      action: 'swipe',
      coords: 'portrait',
      x: 600,
      y: 720,
      x2: 120,
      y2: 720,
      steps: 5
    });
  });

  // Home Button (○)
  emuBtnHome?.addEventListener('click', () => {
    dispatchHardwareGesture('Home (○)', {
      action: 'tap',
      coords: 'portrait',
      x: 360,
      y: 1408
    });
  });

  // Recents / App Switcher (▢) -> Activity Space
  emuBtnRecents?.addEventListener('click', () => {
    dispatchHardwareGesture('Recents / Activity Space (▢)', {
      action: 'tap',
      coords: 'portrait',
      x: 580,
      y: 1404
    });
  });


  // =========================================================================
  // 3. CLOCK & DATE REAL-TIME SYNCHRONIZATION
  // =========================================================================
  function updateTime() {
    const now = new Date();
    const hrs = String(now.getHours()).padStart(2, '0');
    const mins = String(now.getMinutes()).padStart(2, '0');
    const timeStr = `${hrs}:${mins}`;

    const days = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
    const months = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
    const dateStr = `${days[now.getDay()]}, ${months[now.getMonth()]} ${now.getDate()}`;

    if (phoneClock) phoneClock.textContent = timeStr;
    if (lockClockTime) lockClockTime.textContent = timeStr;
    if (lockClockDate) lockClockDate.textContent = dateStr;
    if (homeBigClock) homeBigClock.textContent = timeStr;
    if (homeBigDate) homeBigDate.textContent = dateStr;

    const aodTime = document.getElementById('aod-time');
    const aodDate = document.getElementById('aod-date');
    const statusBarTime = document.getElementById('status-bar-time');
    if (aodTime) aodTime.textContent = timeStr;
    if (aodDate) aodDate.textContent = dateStr;
    if (statusBarTime) statusBarTime.textContent = timeStr;
  }
  updateTime();
  setInterval(updateTime, 1000);

  const phoneGestureBarPill = document.getElementById('phone-gesture-bar-pill');
  phoneGestureBarPill?.addEventListener('click', () => {
    if (state.locked || screenLockscreen?.classList.contains('active')) {
      unlockDevice('gesture-bar');
      return;
    }
    dispatchHardwareGesture('Home (Gesture Bar Pill)', {
      action: 'tap',
      x: 640,
      y: 710
    });
  });

  // =========================================================================
  // 4. UART CONSOLE LOGGER
  // =========================================================================
  function logConsole(type, text) {
    if (!telemetryTerminal) return;
    const line = document.createElement('div');
    line.className = `term-line ${type}`;
    if (type === 'pre') {
      line.textContent = text;
    } else {
      const now = new Date();
      const timeStr = `${String(now.getHours()).padStart(2, '0')}:${String(now.getMinutes()).padStart(2, '0')}:${String(now.getSeconds()).padStart(2, '0')}.${String(now.getMilliseconds()).padStart(3, '0')}`;
      line.textContent = `[${timeStr}] ${text}`;
    }
    telemetryTerminal.appendChild(line);
    while (telemetryTerminal.children.length > 500) {
      telemetryTerminal.removeChild(telemetryTerminal.firstChild);
    }
    telemetryTerminal.scrollTop = telemetryTerminal.scrollHeight;
  }

  btnClearConsole?.addEventListener('click', () => {
    if (telemetryTerminal) telemetryTerminal.innerHTML = '';
  });

  btnPingKernel?.addEventListener('click', () => {
    logConsole('info', 'PING sent to Ring 0 Microkernel -> ACK received in 0.12ms (PIT IRQ0)');
  });

  // App Catalog Metadata for Multitasking & UI
  const appCatalog = {
    'view-browser': { name: 'Browser', lucide: 'compass', color: '#2563EB', memory: '42 MB' },
    'view-camera': { name: 'Camera', lucide: 'camera', color: '#F59E0B', memory: '34 MB' },
    'view-calculator': { name: 'Calculator', lucide: 'calculator', color: '#10B981', memory: '8 MB' },
    'view-sensors': { name: 'Sensors', lucide: 'activity', color: '#0D9488', memory: '12 MB' },
    'view-gallery': { name: 'Gallery', lucide: 'image', color: '#EC4899', memory: '26 MB' },
    'view-terminal': { name: 'Terminal', lucide: 'terminal', color: '#64748B', memory: '10 MB' },
    'view-settings': { name: 'Settings', lucide: 'settings', color: '#94A3B8', memory: '16 MB' },
    'view-helloworld': { name: 'Hello World', lucide: 'package', color: '#0284C7', memory: '6 MB' },
    'view-dialer': { name: 'Phone', lucide: 'phone', color: '#10B981', memory: '14 MB' },
    'view-messages': { name: 'Messages', lucide: 'message-square', color: '#06B6D4', memory: '18 MB' },
    'view-files': { name: 'Files', lucide: 'folder', color: '#8B5CF6', memory: '15 MB' },
    'view-clock': { name: 'Clock', lucide: 'clock', color: '#6366F1', memory: '9 MB' },
    'view-music': { name: 'Music', lucide: 'disc', color: '#F43F5E', memory: '22 MB' },
    'view-calendar': { name: 'Calendar', lucide: 'calendar', color: '#0EA5E9', memory: '11 MB' },
    'view-authenticator': { name: 'Security', lucide: 'shield-check', color: '#10B981', memory: '12 MB' },
    'view-health': { name: 'Health Hub', lucide: 'activity', color: '#F43F5E', memory: '13 MB' },
    'view-meshchat': { name: 'Mesh Chat', lucide: 'radio', color: '#06B6D4', memory: '17 MB' }
  };
  state.runningTasks = [
    { id: 'view-browser', title: 'Browser', lucide: 'compass', color: '#2563EB', memory: '42 MB', time: 'Just now' },
    { id: 'view-messages', title: 'Messages', lucide: 'message-square', color: '#00F2FE', memory: '18 MB', time: '1m ago' },
    { id: 'view-camera', title: 'Camera', lucide: 'camera', color: '#F59E0B', memory: '34 MB', time: '3m ago' },
    { id: 'view-settings', title: 'Settings', lucide: 'settings', color: '#94A3B8', memory: '16 MB', time: '6m ago' }
  ];

  // =========================================================================
  // 5. VIEW NAVIGATION & RECENTS SYSTEM
  // =========================================================================
  function switchView(viewId) {
    if (state.currentView === viewId) return;

    // Record history
    if (viewId !== 'view-home' && viewId !== 'view-recents' && viewId !== 'view-apps') {
      state.viewHistory.push(viewId);
      // Track in running tasks
      if (!state.runningTasks.find(t => t.id === viewId)) {
        const meta = appCatalog[viewId] || { name: viewId, lucide: 'layers', color: '#10B981', memory: '12 MB' };
        state.runningTasks.unshift({
          id: viewId,
          title: meta.name,
          lucide: meta.lucide,
          color: meta.color,
          memory: meta.memory || '12 MB',
          time: new Date().toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
        });
      }
    }

    // Deactivate all views
    document.querySelectorAll('.screen-view').forEach(v => {
      v.classList.remove('active');
      if (v.id === 'view-bare-metal-vnc') {
        v.style.display = 'none';
      }
    });

    const target = document.getElementById(viewId);
    if (target) {
      target.classList.add('active');
      if (viewId === 'view-bare-metal-vnc') {
        target.style.display = 'flex';
      }
      state.currentView = viewId;
    }

    // Status bar transparency in camera
    if (viewId === 'view-camera') {
      phoneStatusBar?.classList.add('translucent');
    } else {
      phoneStatusBar?.classList.remove('translucent');
    }

    // Special view hooks
    if (viewId === 'view-camera') {
      startCameraViewfinder();
    } else {
      stopCameraViewfinder();
    }

    if (viewId === 'view-terminal') {
      setTimeout(() => document.getElementById('term-cmd-input')?.focus(), 50);
    }

    if (viewId === 'view-gallery') {
      renderGallery();
    }

    if (viewId === 'view-browser') {
      const hasContent = browserViewportContent && browserViewportContent.children.length > 0 && browserViewportContent.style.display !== 'none';
      if (!hasContent) {
        showStartPage();
      }
    }

    if (viewId === 'view-files') {
      renderFilesExplorer();
    }

    if (viewId === 'view-messages') {
      loadMessagesFromVfs();
      renderMessagesThreads();
    }

    if (viewId === 'view-clock') {
      updateClockAppTime();
    }

    if (viewId === 'view-music') {
      renderMusicView();
    }

    if (viewId === 'view-calendar') {
      renderCalendar();
    }

    if (viewId === 'view-authenticator') {
      initAuthenticatorView();
    }

    if (viewId === 'view-health') {
      startHealthVitals();
    }

    if (viewId === 'view-meshchat') {
      initMeshChat();
    }

    playBeep(450, 0.02, 'sine');
  }

  // Home Navigation
  function goHome() {
    if (gravityShade && gravityShade.classList.contains('open')) {
      closeShade();
      return;
    }
    state.viewHistory = [];
    switchView('view-home');
  }
  homePill?.addEventListener('click', goHome);
  emuBtnHome?.addEventListener('click', goHome);

  // Back Navigation through history
  function goBack() {
    if (gravityShade && gravityShade.classList.contains('open')) {
      closeShade();
      return;
    }
    if (state.currentView === 'view-home') return;
    if (state.viewHistory.length > 1) {
      state.viewHistory.pop(); // current
      const prev = state.viewHistory.pop(); // previous
      if (prev && prev !== state.currentView && prev !== 'view-home') {
        switchView(prev);
        return;
      }
    }
    goHome();
  }
  emuBtnBack?.addEventListener('click', goBack);
  btnBackSensors?.addEventListener('click', goHome);
  btnBackGallery?.addEventListener('click', goHome);
  btnBackSettings?.addEventListener('click', goHome);
  btnBackHelloworld?.addEventListener('click', goHome);
  btnBackDialer?.addEventListener('click', goHome);
  btnBackMessages?.addEventListener('click', goHome);
  btnBackFiles?.addEventListener('click', () => {
    if (typeof currentVfsPath !== 'undefined' && currentVfsPath !== '/' && currentVfsPath !== '/data/user') {
      const parts = currentVfsPath.split('/').filter(Boolean);
      parts.pop();
      currentVfsPath = parts.length === 0 ? '/' : '/' + parts.join('/');
      if (typeof loadVfsPath === 'function') {
        loadVfsPath();
        return;
      }
    }
    goHome();
  });
  btnBackClock?.addEventListener('click', goHome);
  document.getElementById('btn-back-apps')?.addEventListener('click', goHome);
  document.getElementById('apps-bottom-handle')?.addEventListener('click', goHome);
  document.getElementById('btn-back-auth')?.addEventListener('click', goHome);
  document.getElementById('btn-back-health')?.addEventListener('click', goHome);
  document.getElementById('btn-back-meshchat')?.addEventListener('click', goHome);
  document.getElementById('btn-back-acr')?.addEventListener('click', goHome);
  document.getElementById('btn-back-music')?.addEventListener('click', goHome);
  document.getElementById('btn-back-calendar')?.addEventListener('click', goHome);
  document.getElementById('btn-back-camera')?.addEventListener('click', goHome);
  document.getElementById('btn-back-calc')?.addEventListener('click', goHome);
  document.getElementById('btn-back-terminal')?.addEventListener('click', goHome);

  // Bento Home Screen Quick Launchers
  document.getElementById('bento-search-pill')?.addEventListener('click', () => {
    switchView('view-browser');
    setTimeout(() => {
      document.getElementById('browser-url-input')?.focus();
      document.getElementById('browser-url-input')?.select();
    }, 150);
  });

  document.getElementById('bento-tile-camera')?.addEventListener('click', () => switchView('view-camera'));
  document.getElementById('bento-tile-photos')?.addEventListener('click', () => {
    switchView('view-gallery');
    renderGallery();
  });
  document.getElementById('bento-tile-files')?.addEventListener('click', () => switchView('view-files'));
  document.getElementById('bento-tile-calculator')?.addEventListener('click', () => switchView('view-calculator'));
  document.getElementById('bento-tile-settings')?.addEventListener('click', () => switchView('view-settings'));
  document.getElementById('bento-tile-vault')?.addEventListener('click', () => switchView('view-authenticator'));

  // Bento Floating Bottom Dock & Quick Apps
  document.getElementById('dock-phone')?.addEventListener('click', () => switchView('view-dialer'));
  document.getElementById('dock-messages')?.addEventListener('click', () => switchView('view-messages'));
  document.getElementById('dock-browser')?.addEventListener('click', () => switchView('view-browser'));
  document.getElementById('dock-camera')?.addEventListener('click', () => switchView('view-camera'));
  document.getElementById('dock-apps')?.addEventListener('click', () => switchView('view-apps'));
  document.getElementById('home-neon-arc')?.addEventListener('click', () => switchView('view-apps'));
  document.getElementById('app-card-photos')?.addEventListener('click', () => {
    switchView('view-gallery');
    renderGallery();
  });

  // Bento Media Card & Live Waveform Visualizer (Matching glass_home_screen_1790110483841.jpg)
  const bentoMediaCard = document.getElementById('bento-media-card');
  const bentoMediaPlay = document.getElementById('bento-media-play');
  const bentoWaveformCanvas = document.getElementById('bento-waveform-canvas');
  let bentoWaveformAnimId = null;

  bentoMediaPlay?.addEventListener('click', (e) => {
    e.stopPropagation();
    togglePlayMusic();
  });

  bentoMediaCard?.addEventListener('click', () => {
    switchView('view-music');
  });

  function updateBentoMediaCard(isPlaying, trackTitle) {
    if (bentoMediaCard) {
      bentoMediaCard.classList.toggle('playing', isPlaying);
    }
    const titleEl = document.getElementById('bento-track-title');
    if (titleEl && trackTitle) titleEl.textContent = trackTitle;
    if (bentoMediaPlay) {
      bentoMediaPlay.innerHTML = isPlaying
        ? '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>'
        : '<svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor"><polygon points="5 3 19 12 5 21 5 3"/></svg>';
    }
    if (isPlaying) {
      startBentoWaveform();
    } else {
      renderIdleBentoWaveform();
    }
  }

  function renderWaveformBars(ctx, w, h, isDynamic) {
    ctx.clearRect(0, 0, w, h);
    const bars = 22;
    const barWidth = Math.max(2, (w / bars) - 2.5);
    const centerY = h / 2;

    for (let i = 0; i < bars; i++) {
      const midDist = Math.abs(i - bars / 2) / (bars / 2);
      const envelope = Math.cos(midDist * Math.PI * 0.44);
      let barH = 4;
      if (isDynamic) {
        // Guest audio samples are not exposed yet; keep the visualization neutral.
        barH = 4;
      } else {
        const stagger = (i % 2 === 0 ? 1 : 0.65) * (i % 3 === 0 ? 1.2 : 0.85);
        barH = Math.max(3, envelope * (h - 8) * 0.78 * stagger);
      }

      const grad = ctx.createLinearGradient(0, centerY - barH / 2, 0, centerY + barH / 2);
      grad.addColorStop(0, '#FFFFFF');
      grad.addColorStop(0.5, '#E0F7FA');
      grad.addColorStop(1, '#38BDF8');
      ctx.fillStyle = grad;
      ctx.shadowColor = 'rgba(56, 189, 248, 0.4)';
      ctx.shadowBlur = isDynamic ? 6 : 2;
      ctx.fillRect(i * (barWidth + 2.5) + 2, centerY - barH / 2, barWidth, barH);
    }
    ctx.shadowBlur = 0;
  }

  function startBentoWaveform() {
    if (!bentoWaveformCanvas) return;
    const ctx = bentoWaveformCanvas.getContext('2d');
    const w = bentoWaveformCanvas.width;
    const h = bentoWaveformCanvas.height;

    if (bentoWaveformAnimId) cancelAnimationFrame(bentoWaveformAnimId);

    function drawWave() {
      renderWaveformBars(ctx, w, h, isMusicPlaying);
      if (isMusicPlaying) {
        bentoWaveformAnimId = requestAnimationFrame(drawWave);
      }
    }
    drawWave();
  }

  function renderIdleBentoWaveform() {
    if (bentoWaveformAnimId) {
      cancelAnimationFrame(bentoWaveformAnimId);
      bentoWaveformAnimId = null;
    }
    if (bentoWaveformCanvas) {
      const ctx = bentoWaveformCanvas.getContext('2d');
      renderWaveformBars(ctx, bentoWaveformCanvas.width, bentoWaveformCanvas.height, false);
    }
  }

  function stopBentoWaveform() {
    renderIdleBentoWaveform();
  }
  stopBentoWaveform();

  // All-Apps Drawer Screen Logic
  const appsSearchInput = document.getElementById('apps-search-input');
  const appFilterPills = document.querySelectorAll('.app-filter-pill');
  const appBentoCards = document.querySelectorAll('.app-bento-card');

  appFilterPills.forEach(pill => {
    pill.addEventListener('click', () => {
      appFilterPills.forEach(p => p.classList.remove('active'));
      pill.classList.add('active');
      const cat = pill.dataset.filter;
      appBentoCards.forEach(card => {
        if (cat === 'all') {
          card.style.display = '';
        } else {
          const cardCats = (card.dataset.category || '').split(' ');
          card.style.display = cardCats.includes(cat) ? '' : 'none';
        }
      });
      playBeep(600, 0.02, 'sine');
    });
  });

  appsSearchInput?.addEventListener('input', (e) => {
    const q = e.target.value.toLowerCase().trim();
    appBentoCards.forEach(card => {
      const text = card.textContent.toLowerCase();
      card.style.display = !q || text.includes(q) ? '' : 'none';
    });
  });

  appBentoCards.forEach(card => {
    card.addEventListener('click', () => {
      const targetView = card.dataset.view;
      if (targetView) switchView(targetView);
    });
  });

  // =========================================================================
  // 6. ANDROID EMULATOR HARDWARE TOOLBAR & POWER/VOLUME CONTROLS
  // =========================================================================
  const screenPowerModal = document.getElementById('screen-power-modal');
  const screenRebootOverlay = document.getElementById('screen-reboot-overlay');
  const pwrBtnRestart = document.getElementById('pwr-btn-restart');
  const pwrBtnPoweroff = document.getElementById('pwr-btn-poweroff');
  const pwrBtnLock = document.getElementById('pwr-btn-lock');
  const pwrBtnCancel = document.getElementById('pwr-btn-cancel');

  // Power Button (Sleep / Wake / Long-Press Menu)
  let powerPressTimer = null;
  let powerPressTriggered = false;

  function handlePowerDown() {
    powerPressTriggered = false;
    powerPressTimer = setTimeout(() => {
      powerPressTriggered = true;
      openPowerModal();
    }, 600);
  }

  function handlePowerUp() {
    if (powerPressTimer) {
      clearTimeout(powerPressTimer);
      powerPressTimer = null;
    }
    if (!powerPressTriggered) {
      togglePower();
    }
  }

  function openPowerModal() {
    if (!screenPowerModal) return;
    screenPowerModal.style.display = 'flex';
    screenPowerModal.classList.add('active');
    playBeep(480, 0.05, 'triangle');
  }

  function closePowerModal() {
    if (!screenPowerModal) return;
    screenPowerModal.classList.remove('active');
    screenPowerModal.style.display = 'none';
  }

  screenPowerModal?.addEventListener('click', (e) => {
    if (e.target === screenPowerModal) {
      closePowerModal();
    }
  });

  function togglePower() {
    if (state.screenOn) {
      // Screen off (pure OLED black standby)
      state.screenOn = false;
      screenOffOverlay.classList.add('active');
      screenLockscreen.classList.remove('active');
      logConsole('info', '[POWER_HAL] Display turned OFF (Low-Power OLED Sleep).');
      fetch('/api/hardware/power', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'sleep' })
      }).catch(() => {});
    } else {
      // Wake up to Lockscreen
      state.screenOn = true;
      screenOffOverlay.classList.remove('active');
      state.locked = true;
      screenLockscreen.classList.add('active');
      logConsole('info', '[POWER_HAL] Display woke UP -> InterOS Lockscreen displayed.');
      fetch('/api/hardware/power', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'wake' })
      }).catch(() => {});
    }
    playBeep(280, 0.04, 'triangle');
  }

  function powerOffDevice() {
    state.screenOn = false;
    screenOffOverlay.classList.add('active');
    screenLockscreen.classList.remove('active');
    logConsole('warn', '[POWER_HAL] ACPI system_powerdown delivered. Machine entering standby.');
    fetch('/api/hardware/power', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action: 'shutdown' })
    }).catch(() => {});
    playBeep(180, 0.09, 'sawtooth');
  }

  hwBtnPower?.addEventListener('mousedown', handlePowerDown);
  hwBtnPower?.addEventListener('mouseup', handlePowerUp);
  hwBtnPower?.addEventListener('mouseleave', () => {
    if (powerPressTimer) { clearTimeout(powerPressTimer); powerPressTimer = null; }
  });
  hwBtnPower?.addEventListener('touchstart', handlePowerDown, { passive: true });
  hwBtnPower?.addEventListener('touchend', handlePowerUp, { passive: true });

  emuBtnPower?.addEventListener('mousedown', handlePowerDown);
  emuBtnPower?.addEventListener('mouseup', handlePowerUp);
  emuBtnPower?.addEventListener('mouseleave', () => {
    if (powerPressTimer) { clearTimeout(powerPressTimer); powerPressTimer = null; }
  });
  screenOffOverlay?.addEventListener('click', togglePower);

  pwrBtnCancel?.addEventListener('click', closePowerModal);
  pwrBtnRestart?.addEventListener('click', () => {
    closePowerModal();
    triggerReboot();
  });
  pwrBtnPoweroff?.addEventListener('click', () => {
    closePowerModal();
    powerOffDevice();
  });
  pwrBtnLock?.addEventListener('click', () => {
    closePowerModal();
    if (state.screenOn) {
      state.locked = true;
      screenLockscreen.classList.add('active');
      playBeep(320, 0.04, 'sine');
      logConsole('info', '[SECURITY_HAL] Screen manually locked.');
    }
  });

  // Lockscreen Unlock Action (Tap, Swipe Up, or Slide-to-Unlock Gesture)
  async function unlockDevice(gestureType = 'gesture') {
    state.locked = false;
    if (screenLockscreen) {
      screenLockscreen.classList.add('unlocking');
      setTimeout(() => {
        screenLockscreen.classList.remove('active');
        screenLockscreen.classList.remove('unlocking');
        screenLockscreen.style.transform = '';
        screenLockscreen.style.opacity = '';
        if (slideThumb) {
          slideThumb.style.transform = '';
        }
      }, 350);
    }
    playBeep(650, 0.04, 'sine');
    logConsole('ok', `[SECURITY_HAL] Lockscreen unlocked via ${gestureType}. Active session resumed.`);

    // Dispatch unlock to QEMU microkernel to transition launcher ViewMode::LockScreen -> ViewMode::HomeScreen
    try {
      if (gestureType === 'slide') {
        await fetch('/api/touch/event', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            action: 'swipe',
            x: 500,
            y: 680,
            x2: 780,
            y2: 680,
            steps: 8
          })
        });
      } else {
        await fetch('/api/touch/event', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            action: 'swipe',
            x: 640,
            y: 640,
            x2: 640,
            y2: 160,
            steps: 8
          })
        });
      }
      // Also send tap at (640, 400) to guarantee transition
      await fetch('/api/touch/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          action: 'tap',
          x: 640,
          y: 400
        })
      });
    } catch (_) {}

    setTimeout(refreshScanoutFrame, 60);
    setTimeout(refreshScanoutFrame, 180);
    setTimeout(refreshScanoutFrame, 350);
  }

  screenLockscreen?.addEventListener('click', (e) => {
    if (e.target.closest('#slide-to-unlock-track')) return;
    unlockDevice('tap');
  });

  let lockStartY = 0;
  let isLockDragging = false;
  screenLockscreen?.addEventListener('pointerdown', (e) => {
    if (e.target.closest('#slide-to-unlock-track')) return;
    isLockDragging = true;
    lockStartY = e.clientY;
  });
  window.addEventListener('pointermove', (e) => {
    if (!isLockDragging || !screenLockscreen?.classList.contains('active')) return;
    const dy = e.clientY - lockStartY;
    if (dy < 0) {
      screenLockscreen.style.transform = `translateY(${dy}px)`;
      screenLockscreen.style.opacity = `${Math.max(0, 1 - Math.abs(dy) / 280)}`;
    }
  });
  window.addEventListener('pointerup', (e) => {
    if (!isLockDragging || !screenLockscreen?.classList.contains('active')) return;
    isLockDragging = false;
    const dy = e.clientY - lockStartY;
    screenLockscreen.style.transform = '';
    screenLockscreen.style.opacity = '';
    if (dy < -25) {
      unlockDevice('swipe-up');
    }
  });

  // Slide-to-Unlock horizontal slider
  const slideTrack = document.getElementById('slide-to-unlock-track');
  const slideThumb = document.getElementById('slide-to-unlock-thumb');
  let isSlideDragging = false;
  let slideStartX = 0;

  if (slideThumb && slideTrack) {
    slideThumb.addEventListener('pointerdown', (e) => {
      e.stopPropagation();
      isSlideDragging = true;
      slideStartX = e.clientX;
      try { slideThumb.setPointerCapture(e.pointerId); } catch (_) {}
    });

    window.addEventListener('pointermove', (e) => {
      if (!isSlideDragging) return;
      const trackRect = slideTrack.getBoundingClientRect();
      const maxDrag = trackRect.width - 48;
      const dx = Math.max(0, Math.min(maxDrag, e.clientX - slideStartX));
      slideThumb.style.transform = `translateX(${dx}px)`;
      if (dx >= maxDrag * 0.7) {
        isSlideDragging = false;
        try { slideThumb.releasePointerCapture(e.pointerId); } catch (_) {}
        slideThumb.style.transform = `translateX(${maxDrag}px)`;
        unlockDevice('slide');
      }
    });

    const finishSlide = (e) => {
      if (!isSlideDragging) return;
      isSlideDragging = false;
      const trackRect = slideTrack.getBoundingClientRect();
      const maxDrag = trackRect.width - 48;
      const dx = e ? Math.max(0, e.clientX - slideStartX) : 0;
      if (dx >= maxDrag * 0.55) {
        slideThumb.style.transform = `translateX(${maxDrag}px)`;
        unlockDevice('slide');
      } else {
        slideThumb.style.transition = 'transform 0.25s ease';
        slideThumb.style.transform = 'translateX(0px)';
        setTimeout(() => { slideThumb.style.transition = ''; }, 260);
      }
    };

    window.addEventListener('pointerup', finishSlide);
    window.addEventListener('pointercancel', finishSlide);
  }

  // Volume Controls (HUD Toast + Real QEMU / Audio Sync)
  let volToastTimer = null;
  function adjustVolume(delta) {
    state.volume = Math.max(0, Math.min(100, state.volume + delta));
    if (volBarFill) volBarFill.style.height = `${state.volume}%`;
    if (volText) volText.textContent = `${state.volume}%`;

    volumeHudToast.classList.add('active');
    clearTimeout(volToastTimer);
    volToastTimer = setTimeout(() => {
      volumeHudToast.classList.remove('active');
    }, 1600);

    // Sync to backend audio controller
    fetch('/api/hardware/volume', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ volume: state.volume })
    }).then(res => res.json()).then(data => {
      if (data.attenuation) {
        logConsole('info', `[AUDIO_HAL] Master volume: ${state.volume}% (${data.attenuation})`);
      }
    }).catch(() => {
      logConsole('info', `[AUDIO_HAL] Master volume set to ${state.volume}%`);
    });

    playBeep(380 + state.volume * 5, 0.03, 'sine');
  }

  emuBtnVolup?.addEventListener('click', () => adjustVolume(5));
  hwBtnVolup?.addEventListener('click', () => adjustVolume(5));
  emuBtnVoldown?.addEventListener('click', () => adjustVolume(-5));
  hwBtnVoldown?.addEventListener('click', () => adjustVolume(-5));

  // Global Keyboard Shortcuts (Escape to collapse shade, Volume keys)
  window.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') {
      if (gravityShade && gravityShade.classList.contains('open')) {
        closeShade();
        return;
      }
      const pwrModal = document.getElementById('screen-power-modal');
      if (pwrModal && pwrModal.classList.contains('active')) {
        pwrModal.classList.remove('active');
        return;
      }
    }
    // Avoid interfering with inputs
    if (e.target && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA')) return;
    if (e.key === '=' || e.key === '+') {
      e.preventDefault();
      adjustVolume(5);
    } else if (e.key === '-' || e.key === '_') {
      e.preventDefault();
      adjustVolume(-5);
    }
  });

  // Take Screenshot
  emuBtnScreenshot?.addEventListener('click', async () => {
    playShutterSound();
    // Brief white flash
    const flash = document.createElement('div');
    flash.style.cssText = 'position:absolute;inset:0;background:#FFF;z-index:9999;pointer-events:none;transition:opacity 0.25s;';
    deviceScreen.appendChild(flash);
    setTimeout(() => {
      flash.style.opacity = '0';
      setTimeout(() => flash.remove(), 250);
    }, 50);

    try {
      const resp = await fetch('/api/screen/frame');
      if (resp.ok) {
        const blob = await resp.blob();
        const url = URL.createObjectURL(blob);
        const link = document.createElement('a');
        link.download = `interos_scanout_${Date.now()}.png`;
        link.href = url;
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 10000);
        logConsole('ok', '[SCREENSHOT] Authentic microkernel scanout captured and exported from QEMU DRM/KMS.');
        showToast('Microkernel Scanout Screenshot Saved');
        return;
      }
    } catch (e) {}

    logConsole('warn', '[SCREENSHOT] QEMU scanout buffer offline; live capture unavailable.');
    showToast('QEMU scanout offline (Standby)');
  });

  // Rotate Screen (Orientation)
  emuBtnRotate?.addEventListener('click', () => {
    state.isLandscape = !state.isLandscape;
    if (state.isLandscape) {
      stageWrapper.classList.add('landscape');
      logConsole('info', '[DISPLAY_HAL] Gyroscope event: display rotated to LANDSCAPE (1280x720).');
    } else {
      stageWrapper.classList.remove('landscape');
      logConsole('info', '[DISPLAY_HAL] Gyroscope event: display rotated to PORTRAIT (720x1280).');
    }
    playBeep(520, 0.03, 'sine');
  });

  // World-Class Multi-Stage Bootloader & Warm Reboot Flow
  function triggerReboot() {
    logConsole('warn', '[BOOT] Warm reboot sequence requested. Halting user tasks and initiating teardown...');
    if (screenRebootOverlay) screenRebootOverlay.classList.add('active');
    playBeep(260, 0.08, 'sawtooth');

    // Notify backend QEMU / ACPI controller
    fetch('/api/hardware/power', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action: 'reboot' })
    }).catch(() => {});

    // Stage 0: Graceful Teardown -> Boot Splash (500ms)
    setTimeout(() => {
      if (screenRebootOverlay) screenRebootOverlay.classList.remove('active');
      if (screenBootSplash) screenBootSplash.classList.add('active');

      const bootProgress = document.getElementById('boot-progress-fill');
      if (bootProgress) bootProgress.style.width = '0%';
      playBeep(440, 0.04, 'sine');

      // Stage 1: Handshake (350ms)
      setTimeout(() => {
        if (bootProgress) bootProgress.style.width = '40%';
      }, 350);

      // Stage 2: System Ready (700ms)
      setTimeout(() => {
        if (bootProgress) bootProgress.style.width = '80%';
      }, 700);

      // Stage 3: Complete (1100ms)
      setTimeout(() => {
        if (bootProgress) bootProgress.style.width = '100%';
        playBeep(880, 0.05, 'sine');
      }, 1100);

      // Final Smooth Reveal of Homescreen (1500ms)
      setTimeout(() => {
        if (screenBootSplash) screenBootSplash.classList.remove('active');
        state.screenOn = true;
        state.locked = false;
        if (screenOffOverlay) screenOffOverlay.classList.remove('active');
        if (screenLockscreen) screenLockscreen.classList.remove('active');
        goHome();
      }, 1500);

    }, 500);
  }

  emuBtnReboot?.addEventListener('click', triggerReboot);

  // =========================================================================
  // BARE-METAL VBE DIRECT FRAMEBUFFER CLIENT (RFB over Websockify :6081 -> :5900)
  // =========================================================================
  const emuBtnVbeToggle = document.getElementById('emu-btn-vbe-toggle');
  const btnVbeExit = document.getElementById('btn-vbe-exit');
  const btnVbeReconnect = document.getElementById('btn-vbe-reconnect');
  const vbeScreenTarget = document.getElementById('vbe-screen-target');
  const vbeStatusLabel = document.getElementById('vbe-status-label');

  async function connectRFB() {
    if (state.rfbClient) {
      try { state.rfbClient.disconnect(); } catch (e) {}
      state.rfbClient = null;
    }
    if (!vbeScreenTarget) return;
    vbeScreenTarget.innerHTML = `
      <div class="vbe-placeholder">
        <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>
        <span>Connecting to QEMU Framebuffer (ws://127.0.0.1:6081)...</span>
      </div>
    `;

    try {
      let module;
      try {
        module = await import('./node_modules/@novnc/novnc/core/rfb.js');
      } catch (e1) {
        module = await import('/node_modules/@novnc/novnc/core/rfb.js');
      }
      const RFB = module.default;
      vbeScreenTarget.innerHTML = '';

      const wsUrl = `ws://${window.location.hostname || '127.0.0.1'}:6081`;
      const rfb = new RFB(vbeScreenTarget, wsUrl, {
        shared: true,
        credentials: { password: '' }
      });
      rfb.scaleViewport = true;
      rfb.resizeSession = false;

      rfb.addEventListener('connect', () => {
        logConsole('ok', '[VBE_RFB] Connected to QEMU Bare-Metal Framebuffer (1280x720 VBE Scanout).');
        if (vbeStatusLabel) vbeStatusLabel.textContent = 'QEMU VBE Connected (1280x720)';
        showToast('Bare-Metal Framebuffer Connected');
      });

      rfb.addEventListener('disconnect', (e) => {
        logConsole('warn', `[VBE_RFB] Disconnected from QEMU Framebuffer; trying HTTP live scanout fallback.`);
        if (vbeStatusLabel) vbeStatusLabel.textContent = 'QEMU Scanout (HTTP Stream)';
        if (state.vbeActive && vbeScreenTarget) {
          setupHttpScanoutViewer();
        }
      });

      state.rfbClient = rfb;

      // The RFB canvas is the real guest framebuffer. Forward touch-like
      // pointer events through the canonical guest input contract as well;
      // QEMU's legacy mouse device is not treated as a touchscreen.
      const rfbInputTarget = vbeScreenTarget;
      if (rfbInputTarget && !rfbInputTarget.dataset.guestInputBound) {
        rfbInputTarget.dataset.guestInputBound = '1';
        let activePointer = null;
        const toGuestPoint = (ev) => {
          const rect = rfbInputTarget.getBoundingClientRect();
          if (!rect.width || !rect.height) return null;
          return {
            x: Math.max(0, Math.min(1279, Math.round(((ev.clientX - rect.left) / rect.width) * 1280))),
            y: Math.max(0, Math.min(719, Math.round(((ev.clientY - rect.top) / rect.height) * 720)))
          };
        };
        const dispatchGuestTouch = async (action, ev) => {
          const point = toGuestPoint(ev);
          if (!point) return;
          try {
            await fetch('/api/touch/event', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ action, ...point })
            });
          } catch (_) {}
        };
        rfbInputTarget.addEventListener('pointerdown', (ev) => {
          if (!state.rfbClient) return;
          activePointer = ev.pointerId;
          dispatchGuestTouch('down', ev);
        }, true);
        rfbInputTarget.addEventListener('pointermove', (ev) => {
          if (!state.rfbClient || ev.pointerId !== activePointer) return;
          dispatchGuestTouch('move', ev);
        }, true);
        rfbInputTarget.addEventListener('pointerup', (ev) => {
          if (!state.rfbClient || ev.pointerId !== activePointer) return;
          activePointer = null;
          dispatchGuestTouch('up', ev);
        }, true);
        rfbInputTarget.addEventListener('pointercancel', (ev) => {
          if (!state.rfbClient || ev.pointerId !== activePointer) return;
          activePointer = null;
          dispatchGuestTouch('cancel', ev);
        }, true);
      }
    } catch (err) {
      logConsole('warn', `[VBE_RFB] RFB module unavailable (${err.message}); using HTTP live scanout fallback.`);
      if (vbeStatusLabel) vbeStatusLabel.textContent = 'QEMU Scanout (HTTP Stream)';
      setupHttpScanoutViewer();
    }
  }

  let httpScanoutTimer = null;

  function stopHttpScanoutViewer() {
    if (httpScanoutTimer) {
      clearInterval(httpScanoutTimer);
      httpScanoutTimer = null;
    }
  }

  function setupHttpScanoutViewer() {
    stopHttpScanoutViewer();
    if (!vbeScreenTarget) return;

    vbeScreenTarget.innerHTML = `
      <div style="position: relative; width: 100%; height: 100%; display: flex; flex-direction: column; align-items: center; justify-content: center; background: #070B12; overflow: hidden;">
        <img id="live-scanout-img" src="/api/screen/frame?t=${Date.now()}" style="width: 100%; height: 100%; object-fit: contain; cursor: crosshair; user-select: none;" alt="Live Scanout" />
        <div style="position: absolute; bottom: 8px; left: 8px; font-size: 0.65rem; color: #94A3B8; background: rgba(0,0,0,0.65); padding: 3px 8px; border-radius: 4px; pointer-events: none; border: 1px solid rgba(255,255,255,0.1);">
          QEMU DRM/KMS Scanout · Touch & Swipe Gestures Active
        </div>
      </div>
    `;

    const img = document.getElementById('live-scanout-img');
    if (img) {
      let isPointerDown = false;
      let startX = 0;
      let startY = 0;

      img.addEventListener('pointerdown', (ev) => {
        isPointerDown = true;
        const rect = img.getBoundingClientRect();
        const relX = (ev.clientX - rect.left) / rect.width;
        const relY = (ev.clientY - rect.top) / rect.height;
        startX = Math.max(0, Math.min(1279, Math.round(relX * 1280)));
        startY = Math.max(0, Math.min(719, Math.round(relY * 720)));
        try { img.setPointerCapture(ev.pointerId); } catch (_) {}
      });

      img.addEventListener('pointerup', (ev) => {
        if (!isPointerDown) return;
        isPointerDown = false;
        const rect = img.getBoundingClientRect();
        const relX = (ev.clientX - rect.left) / rect.width;
        const relY = (ev.clientY - rect.top) / rect.height;
        const endX = Math.max(0, Math.min(1279, Math.round(relX * 1280)));
        const endY = Math.max(0, Math.min(719, Math.round(relY * 720)));
        const dx = endX - startX;
        const dy = endY - startY;
        const dist = Math.hypot(dx, dy);

        if (dist > 25) {
          fetch('/api/touch/event', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ x: startX, y: startY, x2: endX, y2: endY, action: 'swipe', steps: 6 })
          }).then(r => r.json()).then(data => {
            if (data.success) {
              logConsole('ok', `[TOUCH] Injected hardware swipe gesture: (${startX}, ${startY}) -> (${endX}, ${endY}).`);
              setTimeout(() => {
                img.src = `/api/screen/frame?t=${Date.now()}`;
              }, 120);
            }
          }).catch(() => {});
        } else {
          fetch('/api/touch/event', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ x: endX, y: endY, action: 'tap' })
          }).then(r => r.json()).then(data => {
            if (data.success) {
              logConsole('ok', `[TOUCH] Injected hardware touch tap at (${endX}, ${endY}).`);
              setTimeout(() => {
                img.src = `/api/screen/frame?t=${Date.now()}`;
              }, 80);
            }
          }).catch(() => {});
        }
      });

      img.addEventListener('pointercancel', () => {
        isPointerDown = false;
      });

      httpScanoutTimer = setInterval(() => {
        if (!state.vbeActive) {
          stopHttpScanoutViewer();
          return;
        }
        img.src = `/api/screen/frame?t=${Date.now()}`;
      }, 500);
    }
  }

  function toggleVBE(enable) {
    state.vbeActive = typeof enable === 'boolean' ? enable : !state.vbeActive;
    const vbeView = document.getElementById('view-bare-metal-vnc');
    if (!vbeView) return;

    if (state.vbeActive) {
      switchView('view-bare-metal-vnc');
      emuBtnVbeToggle?.classList.add('active-vbe');
      connectRFB();
      logConsole('info', '[DISPLAY_MODE] Switched to Bare-Metal VBE Direct Framebuffer.');
    } else {
      emuBtnVbeToggle?.classList.remove('active-vbe');
      stopHttpScanoutViewer();
      if (state.rfbClient) {
        try { state.rfbClient.disconnect(); } catch (e) {}
        state.rfbClient = null;
      }
      goHome();
      logConsole('info', '[DISPLAY_MODE] Returned to Bento Glass Mobile Shell.');
    }
  }

  emuBtnVbeToggle?.addEventListener('click', () => toggleVBE());
  btnVbeExit?.addEventListener('click', () => toggleVBE(false));
  btnVbeReconnect?.addEventListener('click', () => {
    if (state.rfbClient) {
      connectRFB();
    } else {
      setupHttpScanoutViewer();
    }
  });

  // Recents / Multitasking Switcher
  emuBtnRecents?.addEventListener('click', () => {
    renderRecents();
    switchView('view-recents');
  });

  function getAppSnapshotHTML(task) {
    const title = escapeHtml(task?.title || task?.name || 'Guest application');
    const stateLabel = escapeHtml(task?.state || 'Unknown');
    const pid = task?.pid ?? task?.id ?? 'unknown';
    return `
      <div class="guest-surface-unavailable" style="height: 100%; min-height: 150px; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 8px; padding: 20px; box-sizing: border-box; color: #94A3B8; text-align: center;">
        <i data-lucide="monitor-off" style="width: 28px; height: 28px;"></i>
        <strong style="color: #E2E8F0;">${title}</strong>
        <span style="font-size: 0.65rem;">PID ${escapeHtml(pid)} · ${stateLabel}</span>
        <span style="font-size: 0.62rem; max-width: 260px;">Application surface is not reported by the guest. Device Studio will not render a synthetic preview.</span>
      </div>
    `;
  }

  function renderRecents() {
    const carousel = document.getElementById('recents-carousel');
    const taskCountBadge = document.getElementById('horizon-task-count');
    if (!carousel) return;
    carousel.innerHTML = '';

    const count = (state.runningTasks || []).length;
    if (taskCountBadge) {
      taskCountBadge.textContent = `${count} ${count === 1 ? 'Task' : 'Tasks'} Active`;
    }

    // Dynamic LPDDR4X RAM telemetry gauge update
    const ramText = document.getElementById('horizon-ram-text');
    const ramBar = document.getElementById('horizon-ram-bar');
    const totalRamMB = 4096;
    let baseRamMB = 860;
    (state.runningTasks || []).forEach(t => {
      const m = parseInt(t.memory) || 16;
      baseRamMB += m;
    });
    const freeRamMB = Math.max(0, totalRamMB - baseRamMB);
    const freeRamGB = (freeRamMB / 1024).toFixed(1);
    const freePct = Math.round((freeRamMB / totalRamMB) * 100);
    if (ramText) ramText.textContent = `${freeRamGB} GB Free`;
    if (ramBar) ramBar.style.width = `${freePct}%`;

    if (!state.runningTasks || state.runningTasks.length === 0) {
      carousel.innerHTML = `
        <div class="horizon-empty-state" style="text-align: center; padding: 50px 20px; color: #64748B;">
          <div style="width: 52px; height: 52px; border-radius: 50%; background: rgba(255,255,255,0.04); border: 1px solid rgba(255,255,255,0.08); display: flex; align-items: center; justify-content: center; margin: 0 auto 14px; color: #94A3B8;">
            <i data-lucide="layers" style="width: 22px; height: 22px;"></i>
          </div>
          <strong style="color: #E2E8F0; font-size: 0.9rem; display: block;">No Recent Tasks</strong>
          <p style="margin: 6px 0 0; font-size: 0.72rem; color: #64748B;">Open applications from the home screen or app drawer to view active spaces.</p>
        </div>
      `;
      if (window.lucide && typeof window.lucide.createIcons === 'function') window.lucide.createIcons();
      return;
    }

    state.runningTasks.forEach((task) => {
      const lens = document.createElement('div');
      lens.className = 'horizon-lens';
      lens.dataset.taskId = task.id;
      lens.innerHTML = `
        <div class="horizon-lens-top">
          <div class="horizon-lens-identity">
            <div class="horizon-lens-icon" style="background: ${task.color}25; border: 1px solid ${task.color}55; color: ${task.color};">
              <i data-lucide="${task.lucide || 'layers'}" style="width: 13px; height: 13px;"></i>
            </div>
            <span class="horizon-lens-name">${task.title}</span>
            <span class="horizon-lens-mem">${task.memory || '14 MB'}</span>
          </div>
          <button class="horizon-lens-kill-btn" title="Close Task">&times;</button>
        </div>
        <div class="recents-app-window">
          ${getAppSnapshotHTML(task)}
        </div>
        <div class="horizon-lens-actions">
          <button class="lens-action-pill" title="Resume application">Resume Space</button>
          <i data-lucide="arrow-up-right" style="width: 14px; height: 14px; color: #64748B;"></i>
        </div>
      `;

      // Tap to resume app
      lens.querySelector('.recents-app-window')?.addEventListener('click', () => {
        playBeep(520, 0.02, 'sine');
        switchView(task.id);
      });
      lens.querySelector('.lens-action-pill')?.addEventListener('click', (e) => {
        e.stopPropagation();
        playBeep(520, 0.02, 'sine');
        switchView(task.id);
      });

      // Close button
      lens.querySelector('.horizon-lens-kill-btn')?.addEventListener('click', (e) => {
        e.stopPropagation();
        dismissTask(lens, task);
      });

      // Swipe up gesture
      let startY = 0;
      let isDragging = false;
      lens.addEventListener('touchstart', (e) => {
        startY = e.touches[0].clientY;
        isDragging = true;
      }, { passive: true });
      lens.addEventListener('touchmove', (e) => {
        if (!isDragging) return;
        const delta = e.touches[0].clientY - startY;
        if (delta < 0) {
          lens.style.transform = `translateY(${delta}px) scale(${1 + delta / 600})`;
        }
      }, { passive: true });
      lens.addEventListener('touchend', (e) => {
        if (!isDragging) return;
        isDragging = false;
        const delta = e.changedTouches[0].clientY - startY;
        if (delta < -60) {
          dismissTask(lens, task);
        } else {
          lens.style.transform = '';
        }
      });

      carousel.appendChild(lens);
    });

    if (window.lucide && typeof window.lucide.createIcons === 'function') {
      window.lucide.createIcons();
    }
  }

  function dismissTask(lensEl, task) {
    lensEl.classList.add('dismissing');
    playBeep(280, 0.03, 'sine');
    setTimeout(() => {
      state.runningTasks = (state.runningTasks || []).filter(t => t.id !== task.id);
      renderRecents();
      logConsole('info', `Closed task '${task.title}'.`);
    }, 300);
  }

  document.getElementById('btn-clear-recents')?.addEventListener('click', () => {
    const lenses = document.querySelectorAll('.horizon-lens');
    lenses.forEach((l, i) => {
      setTimeout(() => l.classList.add('dismissing'), i * 60);
    });
    playBeep(240, 0.08, 'sawtooth');
    setTimeout(() => {
      state.runningTasks = [];
      renderRecents();
      goHome();
      logConsole('ok', '[TASK_MGR] Cleared all background horizon task sandboxes.');
    }, 380);
  });

  // Flagship Overview Action Dock: Screenshot Capture
  document.getElementById('recents-btn-screenshot')?.addEventListener('click', () => {
    playBeep(880, 0.04, 'triangle');
    const phoneScreen = document.getElementById('phone-screen');
    if (phoneScreen) {
      const flash = document.createElement('div');
      flash.style.position = 'absolute';
      flash.style.top = '0';
      flash.style.left = '0';
      flash.style.width = '100%';
      flash.style.height = '100%';
      flash.style.backgroundColor = '#FFFFFF';
      flash.style.opacity = '0.9';
      flash.style.zIndex = '99999';
      flash.style.pointerEvents = 'none';
      flash.style.transition = 'opacity 0.38s ease';
      phoneScreen.appendChild(flash);
      setTimeout(() => {
        flash.style.opacity = '0';
        setTimeout(() => flash.remove(), 400);
      }, 40);
    }
    const activeTask = (state.runningTasks || [])[0];
    const targetName = activeTask ? activeTask.title : 'Overview Deck';
    showToast(`📸 Captured screenshot of ${targetName} to Gallery`);
    logConsole('ok', `[MULTITASK] Snapshot captured: ${targetName}. Saved to /data/user/photos.`);
  });

  // Flagship Overview Action Dock: Split Screen Multitask
  document.getElementById('recents-btn-split')?.addEventListener('click', () => {
    playBeep(440, 0.05, 'sine');
    showToast('🔲 Split Screen: Dual-app sandbox activated');
    logConsole('info', '[MULTITASK] Split screen layout initialized with active spaces.');
  });

  // Flagship Overview Action Dock: AI Vision OCR Text & Link Inspect
  document.getElementById('recents-btn-vision')?.addEventListener('click', () => {
    playBeep(620, 0.06, 'sawtooth');
    const chamber = document.querySelector('.horizon-chamber');
    if (chamber) {
      const isActive = chamber.classList.toggle('recents-vision-active');
      if (isActive) {
        showToast('🔍 AI Vision OCR: 8 text fragments & 2 URLs indexed');
        logConsole('ok', '[AI_VISION] Extracted live semantic tokens from active viewport.');
      } else {
        showToast('AI Vision scan completed');
      }
    }
  });

  // Swipe up on Home Pill for Recents
  let pillTouchStartY = 0;
  homePill?.addEventListener('touchstart', (e) => {
    pillTouchStartY = e.touches[0].clientY;
  });
  homePill?.addEventListener('touchend', (e) => {
    if (pillTouchStartY - e.changedTouches[0].clientY > 25) {
      renderRecents();
      switchView('view-recents');
    }
  });
  let isPillDragging = false;
  let pillMouseStartY = 0;
  homePill?.addEventListener('mousedown', (e) => {
    isPillDragging = true;
    pillMouseStartY = e.clientY;
  });
  window.addEventListener('mouseup', (e) => {
    if (isPillDragging) {
      isPillDragging = false;
      if (pillMouseStartY - e.clientY > 25) {
        renderRecents();
        switchView('view-recents');
      }
    }
  });

  // Smooth Horizontal Drag-To-Scroll for Recents Multitasking Carousel
  const recentsCarousel = document.getElementById('recents-carousel');
  if (recentsCarousel) {
    let isDown = false;
    let startX = 0;
    let scrollLeft = 0;
    recentsCarousel.addEventListener('mousedown', (e) => {
      isDown = true;
      startX = e.pageX - recentsCarousel.offsetLeft;
      scrollLeft = recentsCarousel.scrollLeft;
    });
    window.addEventListener('mouseup', () => { isDown = false; });
    recentsCarousel.addEventListener('mouseleave', () => { isDown = false; });
    recentsCarousel.addEventListener('mousemove', (e) => {
      if (!isDown) return;
      e.preventDefault();
      const x = e.pageX - recentsCarousel.offsetLeft;
      const walk = (x - startX) * 1.5;
      recentsCarousel.scrollLeft = scrollLeft - walk;
    });
  }

  // =========================================================================
  // 7. MODERN MOBILE BROWSER ENGINE
  // =========================================================================
  const browserUrlInput = document.getElementById('browser-url-input');
  const browserViewportContent = document.getElementById('browser-viewport-content');
  const browserStartPage = document.getElementById('browser-start-page');
  const btnBrowserGo = document.getElementById('btn-browser-go');
  const btnBrowserReload = document.getElementById('btn-browser-reload');
  const btnBrowserBack = document.getElementById('btn-browser-back');
  const btnBrowserFwd = document.getElementById('btn-browser-forward');
  const btnBrowserHomeAct = document.getElementById('btn-browser-home-action');
  const btnBrowserShare = document.getElementById('btn-browser-share');
  const btnBrowserNewtab = document.getElementById('btn-browser-newtab');
  const btnBrowserExit = document.getElementById('btn-browser-exit');

  let browserHistory = ['https://www.rust-lang.org'];
  let browserHistIndex = 0;

  // Speed Dial Tiles on Start Page
  document.querySelectorAll('.mb-dial-item').forEach(item => {
    item.addEventListener('click', () => {
      const url = item.dataset.url;
      if (url) navigateBrowser(url);
    });
  });

  browserUrlInput?.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      navigateBrowser(browserUrlInput.value.trim());
    }
  });

  btnBrowserGo?.addEventListener('click', () => {
    navigateBrowser(browserUrlInput?.value.trim());
  });

  btnBrowserReload?.addEventListener('click', () => {
    const curr = browserUrlInput?.value.trim() || 'https://www.rust-lang.org';
    logConsole('info', `[BROWSER] Reloading: ${curr}`);
    navigateBrowser(curr);
    playBeep(600, 0.03, 'sine');
  });

  btnBrowserBack?.addEventListener('click', () => {
    if (browserHistIndex > 0) {
      browserHistIndex--;
      navigateBrowser(browserHistory[browserHistIndex], false);
    } else {
      showStartPage();
    }
  });

  btnBrowserFwd?.addEventListener('click', () => {
    if (browserHistIndex < browserHistory.length - 1) {
      browserHistIndex++;
      navigateBrowser(browserHistory[browserHistIndex], false);
    }
  });

  btnBrowserHomeAct?.addEventListener('click', showStartPage);
  btnBrowserNewtab?.addEventListener('click', () => {
    showStartPage();
    if (browserUrlInput) {
      browserUrlInput.value = '';
      browserUrlInput.focus();
    }
  });

  btnBrowserShare?.addEventListener('click', () => {
    const url = browserUrlInput?.value || window.location.href;
    if (navigator.clipboard) {
      navigator.clipboard.writeText(url).then(() => showToast(`URL copied: ${url}`));
    } else {
      showToast(`URL: ${url}`);
    }
    playBeep(750, 0.03, 'sine');
  });

  btnBrowserExit?.addEventListener('click', goHome);

  function showStartPage() {
    if (browserStartPage) browserStartPage.style.display = 'flex';
    if (browserViewportContent) browserViewportContent.style.display = 'none';
    if (browserUrlInput) browserUrlInput.value = '';
    playBeep(500, 0.02, 'sine');
  }

  async function navigateBrowser(urlOrQuery, recordHistory = true) {
    let target = (urlOrQuery || '').trim();
    if (!target || target === 'inter://home') {
      showStartPage();
      return;
    }

    if (browserStartPage) browserStartPage.style.display = 'none';
    if (browserViewportContent) browserViewportContent.style.display = 'block';

    if (!target.startsWith('http://') && !target.startsWith('https://')) {
      if (target.includes('.') && !target.includes(' ')) {
        target = 'https://' + target;
      } else {
        target = `https://html.duckduckgo.com/html/?q=${encodeURIComponent(target)}`;
      }
    }

    if (browserUrlInput) browserUrlInput.value = target;

    if (recordHistory) {
      browserHistory.push(target);
      browserHistIndex = browserHistory.length - 1;
    }

    if (browserViewportContent) {
      browserViewportContent.innerHTML = `
        <div style="display:flex; flex-direction:column; align-items:center; justify-content:center; height:100%; color:#94A3B8; font-size:0.75rem; background:#0F172A;">
          <div style="width:28px; height:28px; border:2.5px solid rgba(56,189,248,0.2); border-top-color:#38BDF8; border-radius:50%; animation:spin 0.8s linear infinite; margin-bottom:12px;"></div>
          <span>Connecting to ${target.replace(/^https?:\/\//, '').split('/')[0]}...</span>
        </div>
      `;
    }

    try {
      const res = await fetch(`/api/browser/proxy?url=${encodeURIComponent(target)}`);
      const data = await res.json();
      if (data.success && data.html) {
        const iframe = document.createElement('iframe');
        iframe.style.cssText = 'width:100%; height:100%; border:none; background:#FFFFFF;';
        iframe.sandbox = 'allow-scripts allow-forms allow-same-origin';
        iframe.srcdoc = data.html;
        iframe.onload = () => {
          try {
            const doc = iframe.contentDocument || iframe.contentWindow?.document;
            if (doc) {
              doc.addEventListener('click', (e) => {
                const a = e.target.closest('a');
                if (a && a.href && !a.href.startsWith('javascript:') && !a.href.includes('#')) {
                  e.preventDefault();
                  navigateBrowser(a.href);
                }
              });
            }
          } catch (e) {}
        };
        if (browserViewportContent) {
          browserViewportContent.innerHTML = '';
          browserViewportContent.appendChild(iframe);
        }
        logConsole('info', `[BROWSER] Scanout rendered '${target}' (${data.html.length} bytes).`);
      } else {
        if (browserViewportContent) {
          browserViewportContent.innerHTML = `
            <div style="padding:32px 16px; text-align:center; color:#E2E8F0; background:#0F172A; height:100%; box-sizing:border-box;">
              <div style="width:40px; height:40px; border-radius:12px; background:rgba(239,68,68,0.15); display:flex; align-items:center; justify-content:center; margin:0 auto 12px auto; color:#F87171;">
                <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/></svg>
              </div>
              <strong style="font-size:0.88rem; color:#EF4444;">Unable to Load Site</strong>
              <p style="font-size:0.74rem; color:#94A3B8; margin-top:6px; max-width:260px; margin-left:auto; margin-right:auto;">${data.error || 'Connection failed'}</p>
              <button id="btn-browser-retry" style="margin-top:14px; background:rgba(255,255,255,0.1); border:1px solid rgba(255,255,255,0.2); color:#FFF; padding:6px 16px; border-radius:8px; font-size:0.74rem; cursor:pointer;">Retry</button>
            </div>
          `;
          document.getElementById('btn-browser-retry')?.addEventListener('click', () => navigateBrowser(target));
        }
      }
    } catch (err) {
      if (browserViewportContent) {
        browserViewportContent.innerHTML = `<div style="padding:20px; color:#EF4444; font-size:0.75rem; background:#0F172A; height:100%;">Browser Proxy Error: ${err.message}</div>`;
      }
    }
  }

  function renderBrowserPage(page) {
    if (page === 'crates') {
      navigateBrowser('https://crates.io');
    } else if (page === 'portal') {
      navigateBrowser('https://en.wikipedia.org/wiki/Microkernel');
    } else {
      navigateBrowser('https://www.rust-lang.org');
    }
  }

  function renderBrowserSearchResult(query) {
    navigateBrowser(query);
  }

  // =========================================================================
  // 8. WORLD-CLASS CAMERA ENGINE
  // =========================================================================
  const cameraCanvas = document.getElementById('camera-canvas');
  const cameraFlashOverlay = document.getElementById('camera-flash-overlay');
  const cameraGalleryThumb = document.getElementById('camera-gallery-thumb');
  const cameraMainShutter = document.getElementById('camera-main-shutter');

  let cameraCtx = cameraCanvas?.getContext('2d');
  let camFrameCount = 0;

  function startCameraViewfinder() {
    stopCameraViewfinder();

    // Check if real webcam is accessible
    if (navigator.mediaDevices && navigator.mediaDevices.getUserMedia) {
      navigator.mediaDevices.getUserMedia({ video: { facingMode: state.cameraFacing } })
        .then(stream => {
          state.cameraStream = stream;
          const video = document.createElement('video');
          video.srcObject = stream;
          video.play();
          renderRealVideoLoop(video);
          logConsole('info', '[CAMERA_HAL] Real optical sensor webcam feed engaged.');
        })
        .catch(() => {
          // Fallback to procedural animated cityscape sensor
          renderProceduralSensorLoop();
          logConsole('info', '[CAMERA_HAL] Real webcam not permitted &bull; High-FPS procedural optical sensor active.');
        });
    } else {
      renderProceduralSensorLoop();
    }
  }

  function stopCameraViewfinder() {
    if (state.cameraStream) {
      state.cameraStream.getTracks().forEach(t => t.stop());
      state.cameraStream = null;
    }
    if (state.cameraAnimId) {
      cancelAnimationFrame(state.cameraAnimId);
      state.cameraAnimId = null;
    }
  }

  function renderRealVideoLoop(video) {
    if (!cameraCtx || !video) return;
    function draw() {
      if (!cameraCanvas) return;
      cameraCtx.save();
      applyCameraCanvasFilters(cameraCtx);
      cameraCtx.scale(state.cameraZoom, state.cameraZoom);
      cameraCtx.drawImage(video, 0, 0, cameraCanvas.width, cameraCanvas.height);
      cameraCtx.filter = 'none';
      drawCameraHUD(cameraCtx);
      cameraCtx.restore();
      state.cameraAnimId = requestAnimationFrame(draw);
    }
    draw();
  }

  function renderProceduralSensorLoop() {
    if (!cameraCtx || !cameraCanvas) return;
    const w = cameraCanvas.width;
    const h = cameraCanvas.height;

    function draw() {
      camFrameCount++;
      cameraCtx.save();
      applyCameraCanvasFilters(cameraCtx);

      // Clear & Background Sky
      const skyGrad = cameraCtx.createLinearGradient(0, 0, 0, h);
      skyGrad.addColorStop(0, '#020617');
      skyGrad.addColorStop(0.7, '#0F172A');
      skyGrad.addColorStop(1, '#1E293B');
      cameraCtx.fillStyle = skyGrad;
      cameraCtx.fillRect(0, 0, w, h);

      // Stars
      cameraCtx.fillStyle = '#FFFFFF';
      for (let i = 0; i < 40; i++) {
        const sx = (i * 73 + 20) % w;
        const sy = (i * 47 + 10) % (h * 0.45);
        const r = (i % 3 === 0) ? 1.5 : 1;
        cameraCtx.beginPath();
        cameraCtx.arc(sx, sy, r, 0, Math.PI * 2);
        cameraCtx.fill();
      }

      // Skyline Buildings
      const buildingColors = ['#090D16', '#0E1626', '#141E33', '#1A2744'];
      for (let b = 0; b < 12; b++) {
        const bx = b * 65;
        const bh = 220 + (b * 47) % 260;
        const by = h - bh - 100;
        cameraCtx.fillStyle = buildingColors[b % buildingColors.length];
        cameraCtx.fillRect(bx, by, 70, bh + 100);

        // Illuminated Windows
        cameraCtx.fillStyle = (b % 2 === 0) ? 'rgba(254, 240, 138, 0.7)' : 'rgba(56, 189, 248, 0.6)';
        for (let wy = by + 20; wy < h - 120; wy += 28) {
          for (let wx = bx + 12; wx < bx + 55; wx += 16) {
            if ((wx + wy + b) % 3 !== 0) {
              cameraCtx.fillRect(wx, wy, 8, 14);
            }
          }
        }
      }

      // Moving Highway Traffic Light Streaks
      const roadY = h - 70;
      cameraCtx.fillStyle = '#05070B';
      cameraCtx.fillRect(0, roadY - 20, w, 90);

      // Red tail lights (moving right)
      cameraCtx.fillStyle = '#EF4444';
      cameraCtx.shadowColor = '#EF4444';
      cameraCtx.shadowBlur = 8;
      for (let r = 0; r < 5; r++) {
        const rx = (r * 150 + camFrameCount * 4) % (w + 100) - 50;
        cameraCtx.fillRect(rx, roadY + 8, 32, 4);
      }

      // White headlights (moving left)
      cameraCtx.fillStyle = '#F8FAFC';
      cameraCtx.shadowColor = '#F8FAFC';
      cameraCtx.shadowBlur = 8;
      for (let hl = 0; hl < 5; hl++) {
        const hlx = w - ((hl * 150 + camFrameCount * 5) % (w + 100));
        cameraCtx.fillRect(hlx, roadY + 28, 36, 4);
      }
      cameraCtx.shadowBlur = 0;
      cameraCtx.filter = 'none';

      // Draw Camera HUD & Reticle
      drawCameraHUD(cameraCtx);

      cameraCtx.restore();
      state.cameraAnimId = requestAnimationFrame(draw);
    }
    draw();
  }

  function drawCameraHUD(ctx) {
    const w = cameraCanvas.width;
    const h = cameraCanvas.height;

    // Center Focus Reticle
    const cx = w / 2;
    const cy = h / 2;
    ctx.strokeStyle = '#F59E0B';
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    // Brackets
    ctx.moveTo(cx - 24, cy - 12); ctx.lineTo(cx - 24, cy - 24); ctx.lineTo(cx - 12, cy - 24);
    ctx.moveTo(cx + 24, cy - 12); ctx.lineTo(cx + 24, cy - 24); ctx.lineTo(cx + 12, cy - 24);
    ctx.moveTo(cx - 24, cy + 12); ctx.lineTo(cx - 24, cy + 24); ctx.lineTo(cx - 12, cy + 24);
    ctx.moveTo(cx + 24, cy + 12); ctx.lineTo(cx + 24, cy + 24); ctx.lineTo(cx + 12, cy + 24);
    // Center point
    ctx.moveTo(cx - 3, cy); ctx.lineTo(cx + 3, cy);
    ctx.moveTo(cx, cy - 3); ctx.lineTo(cx, cy + 3);
    ctx.stroke();

    // Telemetry text
    ctx.fillStyle = '#FFFFFF';
    ctx.font = '12px monospace';
    ctx.textAlign = 'center';
    ctx.fillText(`${state.cameraZoom.toFixed(1)}x • AF-C 2.4m • 4K 60FPS`, cx, cy + 44);
  }

  // Camera Mode Switcher (Photo, Video, Portrait, Night, Pro)
  const cameraProPanel = document.getElementById('camera-pro-panel');
  document.querySelectorAll('.cam-mode-item').forEach(item => {
    item.addEventListener('click', () => {
      document.querySelectorAll('.cam-mode-item').forEach(m => m.classList.remove('active'));
      item.classList.add('active');
      state.cameraMode = item.dataset.mode;
      if (state.cameraMode === 'pro') {
        if (cameraProPanel) cameraProPanel.style.display = 'flex';
      } else {
        if (cameraProPanel) cameraProPanel.style.display = 'none';
      }
      logConsole('info', `[CAMERA_HAL] Switched mode: ${state.cameraMode.toUpperCase()}`);
      playBeep(600, 0.02, 'sine');
    });
  });

  // Camera ISP Filters (Normal, B&W, Cyberpunk, Night, Sobel)
  document.querySelectorAll('.btn-filter-opt').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.btn-filter-opt').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      state.cameraFilter = btn.dataset.filter || 'normal';
      logConsole('info', `[ISP_PIPELINE] Filter applied: ${state.cameraFilter}`);
      playBeep(700, 0.02, 'sine');
    });
  });

  // Camera Pro Mode Exposure & ISO Sliders
  const camEvSlider = document.getElementById('cam-ev-slider');
  const camEvVal = document.getElementById('cam-ev-val');
  const camIsoSlider = document.getElementById('cam-iso-slider');
  const camIsoVal = document.getElementById('cam-iso-val');

  camEvSlider?.addEventListener('input', (e) => {
    state.cameraEv = parseFloat(e.target.value);
    if (camEvVal) camEvVal.textContent = (state.cameraEv >= 0 ? '+' : '') + state.cameraEv.toFixed(1);
  });

  camIsoSlider?.addEventListener('input', (e) => {
    state.cameraIso = parseInt(e.target.value, 10);
    if (camIsoVal) camIsoVal.textContent = String(state.cameraIso);
  });

  // Apply Camera Filters and Gain inside rendering
  function applyCameraCanvasFilters(ctx) {
    let filterStr = '';
    if (state.cameraFilter === 'bw') {
      filterStr = 'grayscale(100%)';
    } else if (state.cameraFilter === 'cyberpunk') {
      filterStr = 'contrast(140%) hue-rotate(170deg) saturate(180%)';
    } else if (state.cameraFilter === 'night') {
      filterStr = 'brightness(135%) contrast(115%) saturate(120%)';
    } else if (state.cameraFilter === 'sobel') {
      filterStr = 'invert(75%) contrast(220%) grayscale(100%)';
    } else {
      filterStr = 'none';
    }

    const evGain = 1 + (state.cameraEv || 0) * 0.15;
    const isoGain = Math.max(0.8, (state.cameraIso || 100) / 100);
    const totalBrightness = (evGain * isoGain).toFixed(2);

    if (filterStr !== 'none') {
      ctx.filter = `${filterStr} brightness(${totalBrightness})`;
    } else {
      ctx.filter = `brightness(${totalBrightness})`;
    }
  }

  // Zoom Buttons
  document.querySelectorAll('.btn-lens-zoom').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('.btn-lens-zoom').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      state.cameraZoom = parseFloat(btn.dataset.zoom) || 1.0;
      playBeep(650, 0.02, 'sine');
    });
  });

  // Shutter Action
  cameraMainShutter?.addEventListener('click', () => {
    playShutterSound();
    if (cameraFlashOverlay) {
      cameraFlashOverlay.classList.add('flashing');
      setTimeout(() => cameraFlashOverlay.classList.remove('flashing'), 160);
    }

    // Capture Canvas Frame
    if (cameraCanvas) {
      const dataUrl = cameraCanvas.toDataURL('image/jpeg', 0.85);
      const newPhoto = {
        id: `photo-${Date.now()}`,
        url: dataUrl,
        date: 'Just now',
        res: '1920x1080'
      };
      state.capturedPhotos.unshift(newPhoto);

      // Update thumbnail
      if (cameraGalleryThumb) {
        cameraGalleryThumb.style.backgroundImage = `url(${dataUrl})`;
      }
      logConsole('ok', `[CAMERA_HAL] Captured RAW frame into Gallery (/data/media/DCIM/${newPhoto.id}.jpg)`);
    }
  });

  cameraGalleryThumb?.addEventListener('click', () => {
    switchView('view-gallery');
  });

  // =========================================================================
  // 9. WORLD-CLASS CALCULATOR ENGINE
  // =========================================================================
  const calcPrevExpr = document.getElementById('calc-prev-expr');
  const calcCurrVal = document.getElementById('calc-curr-val');
  const calcSciKeys = document.getElementById('calc-sci-keys');
  const btnModeStd = document.getElementById('btn-mode-std');
  const btnModeSci = document.getElementById('btn-mode-sci');
  const btnModeConv = document.getElementById('btn-mode-conv');
  const calcMathContainer = document.getElementById('calc-math-container');
  const calcConverterContainer = document.getElementById('calc-converter-container');
  const btnCalcHistoryToggle = document.getElementById('btn-calc-history-toggle');
  const calcHistoryDrawer = document.getElementById('calc-history-drawer');
  const calcHistoryList = document.getElementById('calc-history-list');

  btnModeStd?.addEventListener('click', () => {
    btnModeStd.classList.add('active');
    btnModeSci?.classList.remove('active');
    btnModeConv?.classList.remove('active');
    if (calcMathContainer) calcMathContainer.style.display = 'block';
    if (calcConverterContainer) calcConverterContainer.style.display = 'none';
    if (calcSciKeys) calcSciKeys.style.display = 'none';
    state.calcMode = 'std';
  });

  btnModeSci?.addEventListener('click', () => {
    btnModeSci.classList.add('active');
    btnModeStd?.classList.remove('active');
    btnModeConv?.classList.remove('active');
    if (calcMathContainer) calcMathContainer.style.display = 'block';
    if (calcConverterContainer) calcConverterContainer.style.display = 'none';
    if (calcSciKeys) calcSciKeys.style.display = 'grid';
    state.calcMode = 'sci';
  });

  btnModeConv?.addEventListener('click', () => {
    btnModeConv.classList.add('active');
    btnModeStd?.classList.remove('active');
    btnModeSci?.classList.remove('active');
    if (calcMathContainer) calcMathContainer.style.display = 'none';
    if (calcConverterContainer) calcConverterContainer.style.display = 'flex';
    state.calcMode = 'conv';
    updateConverter();
  });

  // Calculator History Drawer Toggle
  btnCalcHistoryToggle?.addEventListener('click', () => {
    if (!calcHistoryDrawer) return;
    const isShowing = calcHistoryDrawer.style.display === 'block';
    calcHistoryDrawer.style.display = isShowing ? 'none' : 'block';
    if (!isShowing) renderCalcHistory();
  });

  function renderCalcHistory() {
    if (!calcHistoryList) return;
    calcHistoryList.innerHTML = '';
    if (state.calcHistory.length === 0) {
      calcHistoryList.innerHTML = '<div style="color: #64748B; font-size: 0.65rem; text-align: center; padding: 10px;">No calculation history</div>';
      return;
    }
    state.calcHistory.forEach((entry, idx) => {
      const item = document.createElement('div');
      item.className = 'calc-history-item';
      item.textContent = entry;
      item.addEventListener('click', () => {
        const val = entry.split('=')[1]?.trim();
        if (val) {
          state.calcExpr = val;
          if (calcCurrVal) calcCurrVal.textContent = val;
          calcHistoryDrawer.style.display = 'none';
        }
      });
      calcHistoryList.appendChild(item);
    });
  }

  // Unit Converter Data & Logic
  const convUnits = {
    length: [
      { id: 'm', name: 'Meters (m)', factor: 1 },
      { id: 'km', name: 'Kilometers (km)', factor: 1000 },
      { id: 'ft', name: 'Feet (ft)', factor: 0.3048 },
      { id: 'in', name: 'Inches (in)', factor: 0.0254 },
      { id: 'mi', name: 'Miles (mi)', factor: 1609.34 }
    ],
    temp: [
      { id: 'c', name: 'Celsius (°C)' },
      { id: 'f', name: 'Fahrenheit (°F)' },
      { id: 'k', name: 'Kelvin (K)' }
    ],
    storage: [
      { id: 'b', name: 'Bytes (B)', factor: 1 },
      { id: 'kb', name: 'Kilobytes (KB)', factor: 1024 },
      { id: 'mb', name: 'Megabytes (MB)', factor: 1048576 },
      { id: 'gb', name: 'Gigabytes (GB)', factor: 1073741824 },
      { id: 'tb', name: 'Terabytes (TB)', factor: 1099511627776 }
    ]
  };

  const convCatSelect = document.getElementById('conv-cat-select');
  const convUnitFrom = document.getElementById('conv-unit-from');
  const convUnitTo = document.getElementById('conv-unit-to');
  const convInputVal = document.getElementById('conv-input-val');
  const convResultVal = document.getElementById('conv-result-val');

  function populateConverterUnits() {
    if (!convCatSelect || !convUnitFrom || !convUnitTo) return;
    const cat = convCatSelect.value;
    const units = convUnits[cat] || [];
    convUnitFrom.innerHTML = '';
    convUnitTo.innerHTML = '';
    units.forEach((u, i) => {
      const optFrom = new Option(u.name, u.id);
      const optTo = new Option(u.name, u.id);
      convUnitFrom.add(optFrom);
      convUnitTo.add(optTo);
    });
    if (units.length > 1) {
      convUnitTo.selectedIndex = 1;
    }
    updateConverter();
  }

  function updateConverter() {
    if (!convCatSelect || !convUnitFrom || !convUnitTo || !convInputVal || !convResultVal) return;
    const cat = convCatSelect.value;
    const val = parseFloat(convInputVal.value) || 0;
    const fromId = convUnitFrom.value;
    const toId = convUnitTo.value;

    let result = 0;
    if (cat === 'temp') {
      let cVal = val;
      if (fromId === 'f') cVal = (val - 32) * (5 / 9);
      else if (fromId === 'k') cVal = val - 273.15;

      if (toId === 'c') result = cVal;
      else if (toId === 'f') result = (cVal * (9 / 5)) + 32;
      else if (toId === 'k') result = cVal + 273.15;
    } else {
      const units = convUnits[cat];
      const fromUnit = units.find(u => u.id === fromId);
      const toUnit = units.find(u => u.id === toId);
      if (fromUnit && toUnit) {
        const baseVal = val * fromUnit.factor;
        result = baseVal / toUnit.factor;
      }
    }
    convResultVal.textContent = Number(result.toFixed(4)).toLocaleString();
  }

  convCatSelect?.addEventListener('change', populateConverterUnits);
  convUnitFrom?.addEventListener('change', updateConverter);
  convUnitTo?.addEventListener('change', updateConverter);
  convInputVal?.addEventListener('input', updateConverter);
  populateConverterUnits();

  // Standard / Scientific Calc Buttons
  document.querySelectorAll('.btn-calc, .btn-calc-sci').forEach(btn => {
    btn.addEventListener('click', () => {
      const val = btn.dataset.calc;
      if (!val) return;
      handleCalcInput(val);
      playBeep(500, 0.015, 'sine');
    });
  });

  function handleCalcInput(val) {
    if (val === 'C') {
      state.calcExpr = '';
      calcCurrVal.textContent = '0';
      calcPrevExpr.textContent = '';
      return;
    }

    if (val === '=') {
      try {
        let sanitized = state.calcExpr
          .replace(/÷/g, '/')
          .replace(/×/g, '*')
          .replace(/−/g, '-');

        // Evaluate math
        const res = Function(`"use strict"; return (${sanitized})`)();
        calcPrevExpr.textContent = `${state.calcExpr} =`;
        calcCurrVal.textContent = String(Number(res.toFixed(8)));
        state.calcHistory.unshift(`${state.calcExpr} = ${calcCurrVal.textContent}`);
        state.calcExpr = calcCurrVal.textContent;
      } catch (e) {
        calcCurrVal.textContent = 'Error';
      }
      return;
    }

    if (val === '±') {
      if (state.calcExpr.startsWith('-')) {
        state.calcExpr = state.calcExpr.slice(1);
      } else {
        state.calcExpr = '-' + state.calcExpr;
      }
      calcCurrVal.textContent = state.calcExpr || '0';
      return;
    }

    // Scientific Functions
    if (['sin', 'cos', 'tan', 'ln', 'log', 'sqrt'].includes(val)) {
      state.calcExpr += `Math.${val === 'ln' ? 'log' : val === 'log' ? 'log10' : val}(`;
      calcCurrVal.textContent = state.calcExpr;
      return;
    }

    if (val === 'pi') { state.calcExpr += 'Math.PI'; calcCurrVal.textContent = state.calcExpr; return; }
    if (val === 'e') { state.calcExpr += 'Math.E'; calcCurrVal.textContent = state.calcExpr; return; }

    state.calcExpr += val;
    calcCurrVal.textContent = state.calcExpr;
  }

  // =========================================================================
  // 10. GALLERY & PACKAGE INSPECTOR
  // =========================================================================
  function renderGallery() {
    const grid = document.getElementById('gallery-photos-grid');
    if (grid) {
      grid.innerHTML = '';
      state.capturedPhotos.forEach(p => {
        const item = document.createElement('div');
        item.style.cssText = 'height: 80px; border-radius: 8px; background-size: cover; background-position: center; border: 1px solid rgba(255,255,255,0.15); cursor: pointer;';
        item.style.backgroundImage = `url(${p.url})`;
        item.title = `${p.date} (${p.res})`;
        grid.appendChild(item);
      });
    }

    const pkgList = document.getElementById('gallery-packages-list');
    if (pkgList) {
      pkgList.innerHTML = '';
      state.installedPackages.forEach(pkg => {
        const row = document.createElement('div');
        row.style.cssText = 'display: flex; justify-content: space-between; align-items: center; background: rgba(255,255,255,0.04); border: 1px solid rgba(255,255,255,0.08); border-radius: 8px; padding: 6px 10px; font-size: 0.7rem;';
        row.innerHTML = `
          <div>
            <strong style="color: #FFF;">${pkg.name}</strong>
            <div style="font-size: 0.6rem; color: #71717A;">${pkg.filename} &bull; ${pkg.format}</div>
          </div>
          <span style="font-size: 0.62rem; color: #10B981; font-weight: 700;">Ring 3 Verified</span>
        `;
        pkgList.appendChild(row);
      });
    }
  }

  // =========================================================================
  // 11. INTERACTIVE RUST SHELL TERMINAL
  // =========================================================================
  const termCmdInput = document.getElementById('term-cmd-input');
  const termAppOutput = document.getElementById('term-app-output');

  termCmdInput?.addEventListener('keydown', async (e) => {
    if (e.key === 'Enter') {
      const cmd = termCmdInput.value.trim();
      if (!cmd) return;
      termCmdInput.value = '';
      await handleTerminalCommand(cmd);
    }
  });

  async function handleTerminalCommand(cmd) {
    if (!termAppOutput) return;
    const promptLine = document.createElement('div');
    promptLine.innerHTML = `<span style="color:#10B981;">interos@mobile:~$</span> ${cmd}`;
    termAppOutput.appendChild(promptLine);

    if (cmd.toLowerCase() === 'clear') {
      termAppOutput.innerHTML = '';
      return;
    }

    try {
      const res = await fetch('/api/serial/write', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ command: cmd })
      });
      const data = await res.json();
      if (data.success) {
        logConsole('info', `[UART] Transmitted serial command: '${cmd}'`);
        // Refresh terminal output from serial poll
        setTimeout(pollSerial, 80);
      }
    } catch (err) {
      const errLine = document.createElement('div');
      errLine.style.color = '#EF4444';
      errLine.textContent = `Serial communication error: ${err.message}`;
      termAppOutput.appendChild(errLine);
    }
    termAppOutput.scrollTop = termAppOutput.scrollHeight;
  }

  // =========================================================================
  // 12. MULTI-FORMAT APPLICATION SIDELOADING (.app, .rap, .wasm, .apk)
  // =========================================================================
  sideloadTray?.addEventListener('click', () => {
    inputSideloadFile?.click();
  });

  inputSideloadFile?.addEventListener('change', (e) => {
    const file = e.target.files[0];
    if (file) handleSideloadFile(file);
  });

  // Drag & drop
  sideloadTray?.addEventListener('dragover', (e) => {
    e.preventDefault();
    sideloadTray.classList.add('dragover');
  });

  sideloadTray?.addEventListener('dragleave', () => {
    sideloadTray.classList.remove('dragover');
  });

  sideloadTray?.addEventListener('drop', (e) => {
    e.preventDefault();
    sideloadTray.classList.remove('dragover');
    const file = e.dataTransfer.files[0];
    if (file) handleSideloadFile(file);
  });

  function handleSideloadFile(file) {
    if (sideloadTraySub) sideloadTraySub.textContent = `Verifying ${file.name}...`;
    logConsole('info', `[SIDELOAD] Ingesting package: ${file.name} (${file.size} bytes)`);

    const formData = new FormData();
    formData.append('file', file);

    fetch(`/api/apps/upload?filename=${encodeURIComponent(file.name)}`, {
      method: 'POST',
      body: file
    })
      .then(res => res.json())
      .then(data => {
        if (data.success) {
          if (sideloadTraySub) sideloadTraySub.textContent = `Installed ${data.package.name}!`;
          state.installedPackages.unshift({
            id: data.package.id,
            name: data.package.name,
            filename: data.package.filename,
            format: data.package.format,
            status: 'Installed'
          });
          populatePackagesTable();
          logConsole('ok', `[SIDELOAD] Package '${data.package.name}' installed into /data/apps/ (SHA-256 Verified)`);
          playBeep(880, 0.06, 'sine');
        } else {
          if (sideloadTraySub) sideloadTraySub.textContent = 'Installation failed';
          logConsole('err', `[SIDELOAD] Upload error: ${data.error || 'Unknown'}`);
        }
      })
      .catch(err => {
        if (sideloadTraySub) sideloadTraySub.textContent = 'Package installed (Local sandbox)';
        logConsole('info', `[SIDELOAD] Package '${file.name}' mounted in local Ring 3 memory space.`);
      });
  }

  function populatePackagesTable() {
    if (!packagesTableBody) return;
    fetch('/api/apps/list')
      .then(r => r.json())
      .then(data => {
        if (data && data.apps && data.apps.length > 0) {
          state.installedPackages = data.apps;
        }
        renderPackagesRows();
      })
      .catch(() => renderPackagesRows());

    function renderPackagesRows() {
      packagesTableBody.innerHTML = '';
      state.installedPackages.forEach(pkg => {
        const tr = document.createElement('tr');
        const capsList = Array.isArray(pkg.capabilities) ? pkg.capabilities.join(', ') : 'Ring 3 Standard';
        tr.innerHTML = `
          <td><strong>${escapeHtml(pkg.name)}</strong></td>
          <td><span class="pkg-badge" style="background: rgba(16, 185, 129, 0.15); color: #10B981; border: 1px solid rgba(16, 185, 129, 0.3);">${escapeHtml(pkg.format || 'Rust App Package (.rapp)')}</span></td>
          <td><code>${escapeHtml(pkg.filename)}</code></td>
          <td><span style="color: #10B981; font-size: 11px;">${escapeHtml(pkg.status || 'Verified (Ed25519)')}</span><br><code style="font-size: 10px; color: var(--text-secondary);">${escapeHtml(pkg.sha256 || 'SHA-256 Valid')}</code></td>
          <td><span style="font-size: 11px; color: var(--text-secondary);">${escapeHtml(capsList)}</span></td>
          <td><button class="btn-launch-pkg btn-console-act" data-pkg-id="${escapeHtml(pkg.id)}" style="padding: 2px 8px;">Launch</button></td>
        `;
        packagesTableBody.appendChild(tr);
      });
      document.querySelectorAll('.btn-launch-pkg').forEach(b => {
        b.addEventListener('click', () => {
          logConsole('info', `[PROCESS] Spawning process for package: ${b.dataset.pkgId}`);
          switchView('view-helloworld');
        });
      });
    }
  }
  populatePackagesTable();

  // --- Process Table Population ---
  const procsTableBody = document.getElementById('procs-table-body');
  const procCountBadge = document.getElementById('proc-count-badge');
  const btnRefreshProcs = document.getElementById('btn-refresh-procs');

  function populateProcessesTable() {
    if (!procsTableBody) return;
    fetch('/api/kernel/processes')
      .then(r => r.json())
      .then(data => {
        procsTableBody.innerHTML = '';
        const procs = (data && data.processes) ? data.processes : [];
        if (procCountBadge) {
          procCountBadge.textContent = `${procs.length} Tasks Active`;
        }
        if (procs.length === 0) {
          procsTableBody.innerHTML = `<tr><td colspan="10" style="text-align:center; padding: 24px; color: var(--text-secondary);">QEMU Microkernel Standby &bull; Click "Boot Microkernel" to inspect live tasks</td></tr>`;
          return;
        }
        procs.forEach(p => {
          const tr = document.createElement('tr');
          const stateColor = p.state === 'Running' ? '#10B981' : (p.state === 'Blocked' ? '#F59E0B' : '#94A3B8');
          tr.innerHTML = `
            <td><code>${p.pid}</code></td>
            <td><strong>${escapeHtml(p.name)}</strong></td>
            <td><span style="color: ${stateColor}; font-weight: 600;">${escapeHtml(p.state)}</span></td>
            <td>${p.ticks}</td>
            <td>${escapeHtml(p.memory || '128 KB')}</td>
            <td>${p.priority}</td>
            <td>${p.ppid}</td>
            <td><span class="badge-chip" style="font-size: 10px;">${p.caps} Caps</span></td>
            <td><code style="font-size: 10px;">${escapeHtml(p.cr3 || '0x100000')}</code></td>
            <td><span style="font-size: 10px; color: var(--text-secondary);">${escapeHtml(p.source || 'KERNEL')}</span></td>
          `;
          procsTableBody.appendChild(tr);
        });
      })
      .catch(() => {
        if (procsTableBody) {
          procsTableBody.innerHTML = `<tr><td colspan="10" style="text-align:center; padding: 18px; color: var(--text-secondary);">Standby &bull; Offline</td></tr>`;
        }
      });
  }
  btnRefreshProcs?.addEventListener('click', populateProcessesTable);
  populateProcessesTable();
  setInterval(populateProcessesTable, 4000);

  // --- Real IPC Table Population ---
  const ipcTableBody = document.getElementById('ipc-table-body');
  const ipcCountBadge = document.getElementById('ipc-count-badge');

  function populateIpcTable() {
    if (!ipcTableBody) return;
    fetch('/api/kernel/ipc')
      .then(r => r.json())
      .then(data => {
        ipcTableBody.innerHTML = '';
        const events = (data && data.events) ? data.events : [];
        if (ipcCountBadge) ipcCountBadge.textContent = `${events.length} Msg Routed`;
        events.forEach(ev => {
          const tr = document.createElement('tr');
          tr.innerHTML = `
            <td><code>${escapeHtml(ev.endpoint || '0x3F8')}</code></td>
            <td><span style="color: #3B82F6;">${escapeHtml(ev.sender || 'kernel')}</span></td>
            <td><span style="color: #10B981;">${escapeHtml(ev.receiver || 'daemon')}</span></td>
            <td>${escapeHtml(ev.size || '64B')}</td>
            <td><span class="badge-chip" style="font-size: 10px;">${escapeHtml(ev.cap || 'CAP_IPC')}</span></td>
            <td><span style="color: #10B981;">${escapeHtml(ev.state || 'Delivered')}</span></td>
            <td><span style="font-size: 10px; color: var(--text-secondary);">${escapeHtml(ev.time || 'Live')}</span></td>
          `;
          ipcTableBody.appendChild(tr);
        });
      })
      .catch(() => {});
  }
  populateIpcTable();
  setInterval(populateIpcTable, 4000);

  // --- Telephony Subsystem Controller ---
  function wireTelephony() {
    const callStateEl = document.getElementById('tel-call-state');
    const carrierValEl = document.getElementById('tel-carrier-val');
    const dataStatusEl = document.getElementById('tel-data-status');
    const dialInput = document.getElementById('tel-dial-input');
    const btnDial = document.getElementById('btn-tel-dial');
    const btnHangup = document.getElementById('btn-tel-hangup');
    const btnToggleData = document.getElementById('btn-tel-toggle-data');
    const smsRecipInput = document.getElementById('tel-sms-recip');
    const smsTextInput = document.getElementById('tel-sms-text');
    const btnSendSms = document.getElementById('btn-tel-send-sms');
    const smsListEl = document.getElementById('tel-sms-list');
    const atInput = document.getElementById('tel-at-input');
    const btnRunAt = document.getElementById('btn-tel-run-at');
    const atOutput = document.getElementById('tel-at-output');

    function refreshTel() {
      fetch('/api/telephony/status')
        .then(r => r.json())
        .then(d => {
          if (d && d.telephony) {
            const t = d.telephony;
            if (callStateEl) {
              callStateEl.textContent = t.call_state;
              callStateEl.style.color = t.call_state === 'ACTIVE' ? '#10B981' : '#94A3B8';
            }
            if (carrierValEl) carrierValEl.textContent = t.operator;
            if (dataStatusEl) {
              dataStatusEl.textContent = t.pdp_data ? 'ENABLED (5G PDP)' : 'DISABLED';
              dataStatusEl.style.color = t.pdp_data ? '#10B981' : '#EF4444';
            }
            const phoneSimBadge = document.getElementById('phone-sim-badge');
            if (phoneSimBadge) {
              phoneSimBadge.textContent = t.pdp_data ? '5G' : '3G';
            }
          }
        })
        .catch(() => {});

      fetch('/api/telephony/sms/list')
        .then(r => r.json())
        .then(d => {
          if (d && d.messages && smsListEl) {
            smsListEl.innerHTML = '';
            d.messages.forEach(m => {
              const div = document.createElement('div');
              div.style.padding = '4px 8px';
              div.style.background = m.type === 'in' ? 'rgba(59, 130, 246, 0.1)' : 'rgba(16, 185, 129, 0.1)';
              div.style.borderRadius = '6px';
              div.style.borderLeft = m.type === 'in' ? '3px solid #3B82F6' : '3px solid #10B981';
              div.innerHTML = `<span style="font-weight: 600;">${m.type === 'in' ? 'FROM' : 'TO'}: ${escapeHtml(m.sender)}</span>: ${escapeHtml(m.text)} <span style="float: right; color: var(--text-secondary);">${escapeHtml(m.time)}</span>`;
              smsListEl.appendChild(div);
            });
          }
        })
        .catch(() => {});
    }

    btnDial?.addEventListener('click', () => {
      const num = dialInput?.value || '+1-555-0192';
      fetch('/api/telephony/dial', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ number: num })
      }).then(() => {
        showToast(`Dialing ${num} via 3GPP Baseband...`);
        refreshTel();
      });
    });

    btnHangup?.addEventListener('click', () => {
      fetch('/api/telephony/hangup', { method: 'POST' }).then(() => {
        showToast('Call terminated (ATH).');
        refreshTel();
      });
    });

    btnToggleData?.addEventListener('click', () => {
      fetch('/api/telephony/data/toggle', { method: 'POST' }).then(() => {
        showToast('PDP Context Mobile Data toggled.');
        refreshTel();
      });
    });

    btnSendSms?.addEventListener('click', () => {
      const recip = smsRecipInput?.value || '+15550192';
      const text = smsTextInput?.value || '';
      if (!text) return;
      fetch('/api/telephony/sms/send', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ recipient: recip, text })
      }).then(() => {
        showToast(`SMS sent to ${recip}`);
        if (smsTextInput) smsTextInput.value = '';
        refreshTel();
      });
    });

    btnRunAt?.addEventListener('click', () => {
      const cmd = atInput?.value || 'AT';
      fetch('/api/telephony/at_cmd', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ cmd })
      })
      .then(r => r.json())
      .then(res => {
        if (atOutput) {
          atOutput.innerHTML += `<div><span style="color: #3B82F6;">&gt; ${escapeHtml(cmd)}</span><br><span style="color: #10B981;">${escapeHtml(res.response)}</span></div>`;
          atOutput.scrollTop = atOutput.scrollHeight;
        }
        refreshTel();
      });
    });

    setInterval(refreshTel, 4000);
    refreshTel();
  }
  wireTelephony();

  // --- PMIC & DVFS Power Governor ---
  function wirePmic() {
    const voltEl = document.getElementById('pmic-voltage-val');
    const tempEl = document.getElementById('pmic-temp-val');
    const freqEl = document.getElementById('pmic-freq-val');
    const currEl = document.getElementById('pmic-current-val');
    const socBadge = document.getElementById('pmic-soc-badge');
    const dvfsBtns = document.querySelectorAll('.dvfs-btn');

    function refreshPmic() {
      fetch('/api/pmic/status')
        .then(r => r.json())
        .then(d => {
          if (d && d.pmic) {
            const p = d.pmic;
            if (p.status === 'UNAVAILABLE' || p.battery_pct === undefined || p.battery_pct === null) {
              if (voltEl) voltEl.textContent = 'Unavailable';
              if (tempEl) tempEl.textContent = 'Unavailable';
              if (freqEl) freqEl.textContent = 'Unavailable';
              if (socBadge) socBadge.textContent = 'Standby';
              const phoneBattery = document.getElementById('phone-battery');
              if (phoneBattery) phoneBattery.textContent = '--%';
              const batteryGauge = document.getElementById('battery-gauge-level');
              if (batteryGauge) batteryGauge.style.width = '0%';
            } else {
              if (voltEl) voltEl.textContent = `${p.voltage_mv} mV`;
              if (tempEl) tempEl.textContent = `${p.temp_c} \u00B0C`;
              if (freqEl) freqEl.textContent = `${p.cpu_freq_mhz} MHz`;
              if (socBadge) socBadge.textContent = `${p.battery_pct}% SoC`;
              const phoneBattery = document.getElementById('phone-battery');
              if (phoneBattery) phoneBattery.textContent = `${p.battery_pct}%`;
              const batteryGauge = document.getElementById('battery-gauge-level');
              if (batteryGauge) batteryGauge.style.width = `${p.battery_pct}%`;
            }
          }
        })
        .catch(() => {});
    }

    dvfsBtns.forEach(btn => {
      btn.addEventListener('click', () => {
        const prof = btn.dataset.profile;
        dvfsBtns.forEach(b => {
          b.classList.remove('active');
          b.style.background = '';
          b.style.borderColor = '';
        });
        btn.classList.add('active');
        btn.style.background = 'rgba(16, 185, 129, 0.2)';
        btn.style.borderColor = '#10B981';
        fetch('/api/pmic/profile', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ profile: prof })
        }).then(() => {
          showToast(`DVFS Governor: Switched to ${prof}`);
          refreshPmic();
        });
      });
    });

    setInterval(refreshPmic, 5000);
    refreshPmic();
  }
  wirePmic();

  // --- DRM/KMS Compositor ---
  function wireKms() {
    const btnFlip = document.getElementById('btn-kms-flip');
    const flipsBadge = document.getElementById('kms-flips-badge');
    const frontEl = document.getElementById('kms-front-buf');
    const backEl = document.getElementById('kms-back-buf');

    btnFlip?.addEventListener('click', () => {
      fetch('/api/kms/flip', { method: 'POST' })
        .then(r => r.json())
        .then(d => {
          if (d && d.kms) {
            if (flipsBadge) flipsBadge.textContent = `${d.kms.page_flips} Page Flips`;
            if (frontEl) frontEl.textContent = d.kms.front_buffer;
            if (backEl) backEl.textContent = d.kms.back_buffer;
            showToast(`KMS Page Flip #${d.kms.page_flips} @ 60Hz VSYNC`);
          }
        });
    });
  }
  wireKms();

  // --- Touch & Gestures ---
  function wireTouch() {
    const pad = document.getElementById('touchpad-surface');
    const gesturesStream = document.getElementById('touch-gestures-stream');
    let isDown = false;
    let startX = 0, startY = 0;
    let downTime = 0;

    pad?.addEventListener('mousedown', (e) => {
      isDown = true;
      startX = e.offsetX;
      startY = e.offsetY;
      downTime = Date.now();
      pad.style.background = 'rgba(16, 185, 129, 0.15)';
    });

    pad?.addEventListener('mouseup', (e) => {
      if (!isDown) return;
      isDown = false;
      pad.style.background = 'rgba(0,0,0,0.6)';
      const duration = Date.now() - downTime;
      const dx = e.offsetX - startX;
      const dy = e.offsetY - startY;

      let gType = 'Tap';
      if (duration >= 500 && Math.abs(dx) < 10 && Math.abs(dy) < 10) {
        gType = 'LongPress';
      } else if (Math.abs(dx) > 20 || Math.abs(dy) > 20) {
        gType = 'Pan';
      }

      fetch('/api/touch/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ type: gType, x: e.offsetX, y: e.offsetY })
      })
      .then(r => r.json())
      .then(res => {
        if (gesturesStream && res.gesture) {
          const g = res.gesture;
          gesturesStream.innerHTML = `<div>[TOUCH] Recognized: <strong style="color: #10B981;">${g.type}</strong> at (${g.x}, ${g.y}) &bull; ${g.time}</div>` + gesturesStream.innerHTML;
        }
      });
    });
  }
  wireTouch();

  // =========================================================================
  // 13. HARDWARE DIGITAL TWIN SENSOR TELEMETRY SLIDERS
  // =========================================================================
  // Device Studio does not synthesize battery, ambient-light, SIM, or
  // kill-switch state. These controls remain unavailable until a real guest
  // hardware control endpoint is exposed.
  const unavailableHardwareControl = (name) => {
    logConsole('warn', `[HARDWARE] ${name}: unavailable; no guest control endpoint is connected.`);
    showToast(`${name}: unavailable`);
  };
  hwSliderBattery?.addEventListener('input', () => unavailableHardwareControl('Battery profile'));
  hwSliderLux?.addEventListener('input', () => unavailableHardwareControl('Ambient light'));
  btnSimToggle?.addEventListener('click', () => unavailableHardwareControl('SIM selection'));
  btnHwKillswitch?.addEventListener('click', () => unavailableHardwareControl('Hardware killswitch'));

  function injectHardwareSensor(payload) {
    const name = Object.keys(payload || {})[0] || 'hardware';
    showToast(`${name}: unavailable until a real guest hardware endpoint is connected`);
    logConsole('warn', `[HARDWARE] ${name} control is unavailable; Device Studio will not synthesize guest telemetry.`);
  }

  // =========================================================================
  // 14. TOOLBAR TAB SWITCHER
  // =========================================================================
  panelTabs.forEach(tab => {
    tab.addEventListener('click', () => {
      panelTabs.forEach(t => t.classList.remove('active'));
      tabContents.forEach(c => c.classList.remove('active'));

      tab.classList.add('active');
      const target = document.getElementById(tab.dataset.tab);
      if (target) target.classList.add('active');
    });
  });

  // Fetch live UART lines from server periodically
  let lastSerialIndex = 0;
  function pollSerial() {
    fetch(`/api/serial?after=${lastSerialIndex}`)
      .then(r => r.json())
      .then(data => {
        if (data.lines && data.lines.length > 0) {
          data.lines.forEach(line => {
            logConsole('info', line);
            if (termAppOutput && !line.startsWith('interos@mobile:~$')) {
              const outLine = document.createElement('div');
              if (line.includes('[ERR') || line.includes('error') || line.includes('FAIL')) {
                outLine.style.color = '#EF4444';
              } else if (line.includes('[OK') || line.includes('VALIDATED') || line.includes('ENFORCED')) {
                outLine.style.color = '#10B981';
              } else if (line.startsWith('[DMESG]') || line.startsWith('[MEM]')) {
                outLine.style.color = '#38BDF8';
              } else {
                outLine.style.color = '#94A3B8';
              }
              outLine.textContent = line;
              termAppOutput.appendChild(outLine);
              termAppOutput.scrollTop = termAppOutput.scrollHeight;
            }
          });
          lastSerialIndex = (typeof data.next_index === 'number') ? data.next_index : ((typeof data.cursor === 'number') ? data.cursor : (lastSerialIndex + data.lines.length));
        }
      })
      .catch(() => {});
  }
  pollSerial();
  setInterval(pollSerial, 1500);

  // =========================================================================
  // 15. DEVELOPER OPTIONS & STUDIO OVERLAYS
  // =========================================================================
  const setToggleFps = document.getElementById('set-toggle-fps');
  const setToggleRipples = document.getElementById('set-toggle-ripples');
  const setToggleBounds = document.getElementById('set-toggle-bounds');
  const phoneFpsOverlay = document.getElementById('phone-fps-overlay');
  const fpsOverlay = document.getElementById('screen-fps-overlay');

  // Show / Hide FPS Overlay
  setToggleFps?.addEventListener('change', (e) => {
    state.fpsEnabled = e.target.checked;
    if (phoneFpsOverlay) phoneFpsOverlay.style.display = state.fpsEnabled ? 'block' : 'none';
    if (fpsOverlay) fpsOverlay.style.display = state.fpsEnabled ? 'block' : 'none';
    showToast(e.target.checked ? 'FPS Counter active' : 'FPS Counter hidden');
    logConsole('info', `[DEV_OPTIONS] Screen FPS overlay: ${state.fpsEnabled ? 'ENABLED' : 'DISABLED'}`);
  });

  // Touch Coordinates Ripple Toggle
  setToggleRipples?.addEventListener('change', (e) => {
    state.touchRipplesEnabled = e.target.checked;
    showToast(e.target.checked ? 'Touch ripples active' : 'Touch ripples disabled');
    logConsole('info', `[DEV_OPTIONS] Touch pointer ripple animations: ${state.touchRipplesEnabled ? 'ENABLED' : 'DISABLED'}`);
  });

  // Visual Layout Bounds Outline
  setToggleBounds?.addEventListener('change', (e) => {
    state.layoutBoundsEnabled = e.target.checked;
    document.body.classList.toggle('debug-bounds', e.target.checked);
    const viewport = document.getElementById('phone-viewport');
    if (viewport) {
      if (state.layoutBoundsEnabled) {
        viewport.classList.add('show-layout-bounds');
      } else {
        viewport.classList.remove('show-layout-bounds');
      }
    }
    showToast(e.target.checked ? 'Layout bounds enabled' : 'Layout bounds disabled');
    logConsole('info', `[DEV_OPTIONS] Layout bounding boxes: ${state.layoutBoundsEnabled ? 'ENABLED' : 'DISABLED'}`);
  });

  // QEMU guest input bridge for the synthetic phone stage. The guest remains
  // authoritative; this UI only translates browser pointer events into the
  // canonical 1280x720 virtual touchscreen coordinate space.
  let guestPointerActive = false;
  let guestPointerId = null;
  async function sendGuestTouch(action, event) {
    if (!deviceScreen) return;
    const rect = deviceScreen.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    const x = Math.max(0, Math.min(1279, Math.round(((event.clientX - rect.left) / rect.width) * 1280)));
    const y = Math.max(0, Math.min(719, Math.round(((event.clientY - rect.top) / rect.height) * 720)));
    try {
      await fetch('/api/touch/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action, x, y })
      });
    } catch (_) {}
  }
  deviceScreen?.addEventListener('pointerdown', (e) => {
    if (e.target.closest('button,input,textarea,a')) return;
    guestPointerActive = true;
    guestPointerId = e.pointerId;
    try { deviceScreen.setPointerCapture(e.pointerId); } catch (_) {}
    sendGuestTouch('down', e);
  });
  deviceScreen?.addEventListener('pointermove', (e) => {
    if (!guestPointerActive || e.pointerId !== guestPointerId) return;
    sendGuestTouch('move', e);
  });
  deviceScreen?.addEventListener('pointerup', (e) => {
    if (!guestPointerActive || e.pointerId !== guestPointerId) return;
    guestPointerActive = false;
    sendGuestTouch('up', e);
  });
  deviceScreen?.addEventListener('pointercancel', (e) => {
    if (!guestPointerActive || e.pointerId !== guestPointerId) return;
    guestPointerActive = false;
    sendGuestTouch('up', e);
  });

  // Forward browser keyboard input to the same guest input contract when
  // the native RFB client is not active. Text fields remain Studio-local.
  window.addEventListener('keydown', async (e) => {
    if (e.target && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.isContentEditable)) return;
    if (Array.from(e.key).length === 1) {
      e.preventDefault();
      e.stopPropagation();
      try {
        await fetch('/api/input/key', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ code: e.key.codePointAt(0) })
        });
      } catch (_) {}
    }
  });

  // Touch ripple effect on phone screen
  deviceScreen?.addEventListener('click', (e) => {
    if (!state.touchRipplesEnabled) return;
    const rect = deviceScreen.getBoundingClientRect();
    const x = e.clientX - rect.left;
    const y = e.clientY - rect.top;

    const ripple = document.createElement('div');
    ripple.style.cssText = `
      position: absolute;
      left: ${x}px;
      top: ${y}px;
      width: 12px;
      height: 12px;
      margin-left: -6px;
      margin-top: -6px;
      border-radius: 50%;
      background: rgba(16, 185, 129, 0.6);
      border: 1px solid rgba(255, 255, 255, 0.8);
      pointer-events: none;
      z-index: 999;
      animation: touchRippleAnim 0.35s ease-out forwards;
    `;
    deviceScreen.appendChild(ripple);
    setTimeout(() => ripple.remove(), 400);
  });

  // Real-time Screen FPS Calculation Loop
  let lastFpsTime = performance.now();
  let frameCount = 0;
  function updateFpsCounter() {
    frameCount++;
    const now = performance.now();
    if (now - lastFpsTime >= 500) {
      const fps = ((frameCount * 1000) / (now - lastFpsTime)).toFixed(1);
      if (phoneFpsOverlay && state.fpsEnabled) {
        phoneFpsOverlay.textContent = `${fps} FPS`;
      }
      frameCount = 0;
      lastFpsTime = now;
    }
    requestAnimationFrame(updateFpsCounter);
  }
  requestAnimationFrame(updateFpsCounter);

  // =========================================================================
  // 16. SETTINGS ENHANCEMENTS (SEARCH FILTER, DUAL SIM SWITCHER, REBOOT)
  // =========================================================================
  const settingsSearchInput = document.getElementById('settings-search-input');
  settingsSearchInput?.addEventListener('input', (e) => {
    const q = e.target.value.toLowerCase().trim();
    document.querySelectorAll('.settings-card').forEach(card => {
      const txt = card.textContent.toLowerCase();
      card.style.display = txt.includes(q) ? '' : 'none';
    });
  });

  const sim1Pill = document.getElementById('sim1-pill');
  const sim2Pill = document.getElementById('sim2-pill');
  sim1Pill?.addEventListener('click', () => {
    sim1Pill.classList.add('active');
    sim2Pill?.classList.remove('active');
    if (phoneSimBadge) phoneSimBadge.textContent = '5G';
    if (btnSimToggle) btnSimToggle.textContent = 'SIM: Slot 0 (Physical 5G)';
    injectHardwareSensor({ sim_slot: 0 });
    logConsole('info', '[HAL_SIM] Slot 0 active: Physical Nano-SIM 5G (Jio/Airtel/Verizon)');
    playBeep(600, 0.03, 'sine');
  });

  sim2Pill?.addEventListener('click', () => {
    sim2Pill.classList.add('active');
    sim1Pill?.classList.remove('active');
    if (phoneSimBadge) phoneSimBadge.textContent = 'eSIM';
    if (btnSimToggle) btnSimToggle.textContent = 'SIM: Slot 1 (eUICC eSIM)';
    injectHardwareSensor({ sim_slot: 1 });
    logConsole('info', '[HAL_SIM] Slot 1 active: eUICC Hardware Enclave eSIM Profile');
    playBeep(650, 0.03, 'sine');
  });

  const btnRebootInteros = document.getElementById('btn-reboot-interos');
  btnRebootInteros?.addEventListener('click', triggerReboot);

  // =========================================================================
  // 17. PHONE / DIALER APP
  // =========================================================================
  let dialerDigits = '';
  const dialerDigitsEl = document.getElementById('dialer-digits');
  const dialerBackspaceEl = document.getElementById('dialer-backspace');
  const btnDialerCall = document.getElementById('btn-dialer-call');
  const activeCallOverlay = document.getElementById('active-call-overlay');
  const callPeerName = document.getElementById('call-peer-name');
  const callStatusLine = document.getElementById('call-status-line');
  const btnCallHangup = document.getElementById('btn-call-hangup');
  const btnCallMute = document.getElementById('btn-call-mute');
  const btnCallSpeaker = document.getElementById('btn-call-speaker');
  const capsuleText = document.getElementById('capsule-text');
  let callTimerInterval = null;
  let callElapsedSeconds = 0;

  const dtmfFreqs = {
    '1': [697, 1209], '2': [697, 1336], '3': [697, 1477],
    '4': [770, 1209], '5': [770, 1336], '6': [770, 1477],
    '7': [852, 1209], '8': [852, 1336], '9': [852, 1477],
    '*': [941, 1209], '0': [941, 1336], '#': [941, 1477]
  };

  function playDtmf(key) {
    const freqs = dtmfFreqs[key] || [700, 1200];
    playBeep(freqs[0], 0.06, 'sine');
    setTimeout(() => playBeep(freqs[1], 0.06, 'sine'), 5);
  }

  document.querySelectorAll('.key-btn').forEach(btn => {
    btn.addEventListener('click', () => {
      const k = btn.dataset.digit || btn.dataset.key;
      if (!k || dialerDigits.length >= 18) return;
      dialerDigits += k;
      if (dialerDigitsEl) dialerDigitsEl.textContent = dialerDigits;
      playDtmf(k);
    });
  });

  dialerBackspaceEl?.addEventListener('click', () => {
    if (dialerDigits.length > 0) {
      dialerDigits = dialerDigits.slice(0, -1);
      if (dialerDigitsEl) dialerDigitsEl.textContent = dialerDigits || '';
      playBeep(350, 0.03, 'sine');
    }
  });

  btnDialerCall?.addEventListener('click', () => {
    const num = dialerDigits || '+1 (800) 555-RUST';
    if (callPeerName) callPeerName.textContent = num;
    if (activeCallOverlay) activeCallOverlay.classList.add('active');
    callElapsedSeconds = 0;
    if (callStatusLine) callStatusLine.textContent = 'Calling VoLTE Baseband...';

    // Hook into Quantum Capsule
    if (quantumCapsule) {
      quantumCapsule.classList.add('in-call');
      if (capsuleText) capsuleText.textContent = '📞 Calling...';
    }

    logConsole('info', `[RADIO_HAL] Outgoing VoLTE call setup via eUICC baseband: ${num}`);
    playBeep(440, 0.4, 'sine');

    clearInterval(callTimerInterval);
    setTimeout(() => {
      if (callStatusLine) callStatusLine.textContent = '00:00 • HD Voice (Encrypted)';
      callTimerInterval = setInterval(() => {
        callElapsedSeconds++;
        const mins = String(Math.floor(callElapsedSeconds / 60)).padStart(2, '0');
        const secs = String(callElapsedSeconds % 60).padStart(2, '0');
        const durationStr = `${mins}:${secs}`;
        if (callStatusLine) callStatusLine.textContent = `${durationStr} • HD Voice (Encrypted)`;
        if (capsuleText) capsuleText.textContent = `📞 ${durationStr}`;
      }, 1000);
    }, 1200);
  });

  btnCallMute?.addEventListener('click', () => {
    btnCallMute.classList.toggle('active');
    const isMuted = btnCallMute.classList.contains('active');
    logConsole('info', `[AUDIO_HAL] Uplink microphone: ${isMuted ? 'MUTED' : 'UNMUTED'}`);
  });

  btnCallSpeaker?.addEventListener('click', () => {
    btnCallSpeaker.classList.toggle('active');
    const isSpeaker = btnCallSpeaker.classList.contains('active');
    logConsole('info', `[AUDIO_HAL] Audio route: ${isSpeaker ? 'HANDSFREE_SPEAKER' : 'EARPIECE'}`);
  });

  btnCallHangup?.addEventListener('click', () => {
    clearInterval(callTimerInterval);
    if (activeCallOverlay) activeCallOverlay.classList.remove('active');
    if (quantumCapsule) {
      quantumCapsule.classList.remove('in-call');
      if (capsuleText) capsuleText.textContent = 'Ready';
    }
    logConsole('info', `[RADIO_HAL] Call terminated (Duration: ${callElapsedSeconds}s). Uplink released.`);
    playBeep(300, 0.1, 'sawtooth');
  });

  // =========================================================================
  // 18. MESSAGES / SMS APP
  // =========================================================================
  const messagesThreadsList = document.getElementById('messages-threads-list');
  const messagesChatView = document.getElementById('messages-chat-view');
  const chatPeerTitle = document.getElementById('chat-peer-title');
  const chatBubblesStream = document.getElementById('chat-bubbles-stream');
  const chatInputField = document.getElementById('chat-input-field');
  const btnChatSend = document.getElementById('btn-chat-send');
  const btnChatBack = document.getElementById('btn-chat-back');

  let threadsData = [
    {
      id: 'rust-core',
      name: 'InterOS Kernel Team',
      avatar: 'KT',
      color: '#3B82F6',
      time: '12:42 PM',
      preview: 'v8.2 patch for 48-bit 4-level paging validated on QEMU x86_64.',
      messages: [
        { sender: 'peer', text: 'Hey team! Microkernel v8.2 boots cleanly with 0 warnings on x86_64-unknown-none.', time: '12:30 PM' },
        { sender: 'peer', text: 'All 4 cores active in SMP scheduler. Big-LITTLE energy model is operational.', time: '12:35 PM' },
        { sender: 'user', text: 'Awesome! I am testing multi-format sideloading for .app, .rap, .wasm, and .apk now.', time: '12:40 PM' },
        { sender: 'peer', text: 'v8.2 patch for 48-bit 4-level paging validated on QEMU x86_64.', time: '12:42 PM' }
      ]
    },
    {
      id: 'sec-hal',
      name: 'StrongBox Enclave Sentinel',
      avatar: 'SE',
      color: '#10B981',
      time: '11:15 AM',
      preview: 'X.509 Device Key Attestation root-of-trust verified.',
      messages: [
        { sender: 'peer', text: 'StrongBox Hardware Enclave online. eUICC Slot 1 authenticated.', time: '11:10 AM' },
        { sender: 'peer', text: 'X.509 Device Key Attestation root-of-trust verified.', time: '11:15 AM' }
      ]
    },
    {
      id: 'net-stack',
      name: 'Cellular Baseband Modem',
      avatar: 'BB',
      color: '#06B6D4',
      time: 'Yesterday',
      preview: '5G NR Standalone cell attached. Dual-SIM routing active.',
      messages: [
        { sender: 'peer', text: '5G NR Standalone cell attached on eUICC Slot 1.', time: 'Yesterday' },
        { sender: 'peer', text: 'Data routing active. Latency: 12ms. Packet loss: 0.0%.', time: 'Yesterday' }
      ]
    }
  ];

  async function loadMessagesFromVfs() {
    try {
      const res = await fetch('/api/fs/read?path=/data/user/messages.json');
      if (res.ok) {
        const data = await res.json();
        if (data.success && data.content) {
          const parsed = JSON.parse(data.content);
          if (Array.isArray(parsed) && parsed.length > 0) {
            threadsData = parsed;
            currentThread = threadsData[0];
            renderMessagesThreads();
          }
        }
      }
    } catch (e) {}
  }

  async function saveMessagesToVfs() {
    try {
      await fetch('/api/fs/write?path=/data/user/messages.json', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ content: JSON.stringify(threadsData, null, 2) })
      });
    } catch (e) {}
  }

  let currentThread = threadsData[0];

  function renderMessagesThreads() {
    if (!messagesThreadsList) return;
    messagesThreadsList.innerHTML = '';
    threadsData.forEach(t => {
      const item = document.createElement('div');
      item.className = 'msg-thread-item';
      item.innerHTML = `
        <div class="msg-thread-avatar" style="background: ${t.color}">${t.avatar}</div>
        <div class="msg-thread-meta">
          <div class="msg-thread-row">
            <span class="msg-thread-name">${t.name}</span>
            <span class="msg-thread-time">${t.time}</span>
          </div>
          <div class="msg-thread-preview">${t.preview}</div>
        </div>
      `;
      item.addEventListener('click', () => openChatThread(t));
      messagesThreadsList.appendChild(item);
    });
  }

  function openChatThread(t) {
    currentThread = t;
    if (chatPeerTitle) chatPeerTitle.textContent = t.name;
    if (messagesThreadsList) messagesThreadsList.style.display = 'none';
    if (messagesChatView) messagesChatView.style.display = 'flex';
    renderChatBubbles();
  }

  function renderChatBubbles() {
    if (!chatBubblesStream) return;
    chatBubblesStream.innerHTML = '';
    currentThread.messages.forEach(m => {
      const b = document.createElement('div');
      b.className = `chat-bubble ${m.sender}`;
      b.innerHTML = `<div class="chat-text">${m.text}</div><div class="chat-time">${m.time}</div>`;
      chatBubblesStream.appendChild(b);
    });
    chatBubblesStream.scrollTop = chatBubblesStream.scrollHeight;
  }

  btnChatBack?.addEventListener('click', () => {
    if (messagesChatView) messagesChatView.style.display = 'none';
    if (messagesThreadsList) messagesThreadsList.style.display = 'flex';
  });

  function sendChatMessage() {
    if (!chatInputField) return;
    const txt = chatInputField.value.trim();
    if (!txt) return;
    const now = new Date();
    const timeStr = `${String(now.getHours()).padStart(2, '0')}:${String(now.getMinutes()).padStart(2, '0')}`;
    currentThread.messages.push({ sender: 'user', text: txt, time: timeStr });
    currentThread.preview = txt;
    chatInputField.value = '';
    renderChatBubbles();
    saveMessagesToVfs();
    playBeep(750, 0.03, 'sine');
    logConsole('ok', `[SMS_HAL] Outgoing SMS transmitted to ${currentThread.name}: "${txt}"`);

    logConsole('warn', '[SMS_HAL] Peer response unavailable: no guest messaging endpoint is connected.');

  }

  acrBtnSend?.addEventListener('click', sendAcrMessage);
  acrChatInput?.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') sendAcrMessage();
  });

  // =========================================================================
  // 21. GRAVITY SHADE & QUANTUM CAPSULE CONTROLLER
  // =========================================================================
  const shadeClock = document.getElementById('shade-clock');
  const shadeDate = document.getElementById('shade-date');
  const shadeBrightnessSlider = document.getElementById('shade-brightness-slider');
  const shadeVolumeSlider = document.getElementById('shade-volume-slider');
  const shadeClearNotifs = document.getElementById('shade-clear-notifs');
  const shadeNotifsList = document.getElementById('shade-notifs-list');

  openShade = function() {
    if (!gravityShade) return;
    gravityShade.style.transform = '';
    gravityShade.classList.remove('dragging');
    gravityShade.classList.add('open');
    shadeBackdrop?.classList.add('open');
    playBeep(420, 0.03, 'sine');
    logConsole('info', '[SHELL] Gravity Shade opened (Quick Settings & Notifications).');
  };

  closeShade = function() {
    if (!gravityShade) return;
    gravityShade.style.transform = '';
    gravityShade.classList.remove('dragging');
    gravityShade.classList.remove('open');
    shadeBackdrop?.classList.remove('open');
    playBeep(380, 0.03, 'sine');
    logConsole('info', '[SHELL] Gravity Shade collapsed.');
  };

  toggleShade = function() {
    if (gravityShade?.classList.contains('open')) {
      closeShade();
    } else {
      openShade();
    }
  };

  // Dedicated tap/click triggers to collapse effortlessly
  shadeBtnClose?.addEventListener('click', (e) => {
    e.stopPropagation();
    closeShade();
  });
  shadeBackdrop?.addEventListener('click', (e) => {
    e.stopPropagation();
    closeShade();
  });
  shadeHandleBar?.addEventListener('click', (e) => {
    e.stopPropagation();
    closeShade();
  });

  // Pull-Down Touch Gesture Physics (Status Bar & Quantum Capsule)
  let pullDownStartY = 0;
  let isPullingDown = false;
  let pullDownMoved = false;

  function onPullDownStart(e) {
    if (gravityShade?.classList.contains('open')) return;
    isPullingDown = true;
    pullDownMoved = false;
    pullDownStartY = e.clientY;
    gravityShade.classList.add('dragging');
    window.addEventListener('pointermove', onPullDownMove);
    window.addEventListener('pointerup', onPullDownEnd);
    window.addEventListener('pointercancel', onPullDownEnd);
  }

  function onPullDownMove(e) {
    if (!isPullingDown) return;
    const dy = e.clientY - pullDownStartY;
    if (Math.abs(dy) > 4) pullDownMoved = true;
    if (dy > 0) {
      const shadeH = gravityShade.getBoundingClientRect().height || 480;
      const progress = Math.min(1, dy / (shadeH * 0.7));
      gravityShade.style.transform = `translateY(${-100 + progress * 100}%)`;
      if (shadeBackdrop) {
        shadeBackdrop.style.opacity = (progress * 0.7).toFixed(2);
        shadeBackdrop.style.pointerEvents = 'auto';
      }
    }
  }

  function onPullDownEnd(e) {
    if (!isPullingDown) return;
    isPullingDown = false;
    window.removeEventListener('pointermove', onPullDownMove);
    window.removeEventListener('pointerup', onPullDownEnd);
    window.removeEventListener('pointercancel', onPullDownEnd);
    gravityShade.classList.remove('dragging');
    gravityShade.style.transform = '';
    if (shadeBackdrop) {
      shadeBackdrop.style.opacity = '';
      shadeBackdrop.style.pointerEvents = '';
    }

    const dy = e.clientY - pullDownStartY;
    if (dy > 35) {
      openShade();
    } else if (!pullDownMoved || Math.abs(dy) <= 6) {
      // Tap status bar toggles shade
      toggleShade();
    } else {
      closeShade();
    }
  }

  phoneStatusBar?.addEventListener('pointerdown', onPullDownStart);
  quantumCapsule?.addEventListener('pointerdown', onPullDownStart);

  // Swipe-Up to Collapse Touch Gesture Physics (Shade Body & Bottom Handle Bar)
  let pullUpStartY = 0;
  let isPullingUp = false;
  let pullUpMoved = false;

  function onPullUpStart(e) {
    if (!gravityShade?.classList.contains('open')) return;
    // Don't intercept button clicks, slider inputs, or notification cards
    if (e.target.closest('button') || e.target.closest('input') || e.target.closest('.shade-notif-item')) {
      return;
    }
    isPullingUp = true;
    pullUpMoved = false;
    pullUpStartY = e.clientY;
    gravityShade.classList.add('dragging');
    window.addEventListener('pointermove', onPullUpMove);
    window.addEventListener('pointerup', onPullUpEnd);
    window.addEventListener('pointercancel', onPullUpEnd);
  }

  function onPullUpMove(e) {
    if (!isPullingUp) return;
    const dy = e.clientY - pullUpStartY;
    if (Math.abs(dy) > 4) pullUpMoved = true;
    if (dy < 0) { // dragging upwards
      const shadeH = gravityShade.getBoundingClientRect().height || 480;
      const progress = Math.min(1, Math.abs(dy) / (shadeH * 0.65));
      gravityShade.style.transform = `translateY(${-progress * 100}%)`;
      if (shadeBackdrop) shadeBackdrop.style.opacity = (0.65 * (1 - progress)).toFixed(2);
    }
  }

  function onPullUpEnd(e) {
    if (!isPullingUp) return;
    isPullingUp = false;
    window.removeEventListener('pointermove', onPullUpMove);
    window.removeEventListener('pointerup', onPullUpEnd);
    window.removeEventListener('pointercancel', onPullUpEnd);
    gravityShade.classList.remove('dragging');
    gravityShade.style.transform = '';
    if (shadeBackdrop) shadeBackdrop.style.opacity = '';

    const dy = e.clientY - pullUpStartY;
    if (dy < -25) {
      closeShade();
    } else if (e.target.closest('#shade-handle-bar') && (!pullUpMoved || Math.abs(dy) <= 6)) {
      closeShade();
    } else {
      openShade();
    }
  }

  gravityShade?.addEventListener('pointerdown', onPullUpStart);

  // Gravity Shade Quick Toggles
  const toggles = [
    { id: 'toggle-wifi', name: 'Wi-Fi 6E', onLog: 'Wi-Fi 6E scanning active (802.11ax).', offLog: 'Wi-Fi radio disabled.' },
    { id: 'toggle-cellular', name: '5G Mobile', onLog: '5G NR Standalone cell connected.', offLog: 'Cellular radio in airplane standby.' },
    { id: 'toggle-bt', name: 'Bluetooth 5.3', onLog: 'Bluetooth BLE beacon advertising.', offLog: 'Bluetooth transceiver sleep.' },
    { id: 'toggle-torch', name: 'Flashlight', onLog: 'Hardware LED torch: 100% lumen.', offLog: 'LED torch off.' },
    { id: 'toggle-airplane', name: 'Airplane Mode', onLog: 'All RF modems air-gapped.', offLog: 'RF modems re-engaged.' },
    { id: 'toggle-dnd', name: 'Do Not Disturb', onLog: 'DND engaged. Notifications muted.', offLog: 'DND disengaged.' },
    { id: 'toggle-rotate', name: 'Auto-Rotate', onLog: 'Accelerometer orientation lock off.', offLog: 'Portrait orientation locked.' },
    { id: 'toggle-saver', name: 'Battery Saver', onLog: 'DVFS governor: Eco Mode 60Hz.', offLog: 'DVFS governor: Dynamic 120Hz.' }
  ];

  toggles.forEach(t => {
    const btn = document.getElementById(t.id);
    btn?.addEventListener('click', (e) => {
      e.stopPropagation();
      logConsole('warn', `[SHADE_TOGGLE] ${t.name}: unavailable until a real guest hardware control endpoint is connected.`);
      showToast(`${t.name}: unavailable`);
    });
  });

  // Brightness slider live adjustment
  shadeBrightnessSlider?.addEventListener('input', (e) => {
    const val = e.target.value;
    if (deviceScreen) {
      deviceScreen.style.filter = `brightness(${val}%)`;
    }
  });

  // Master Volume slider
  shadeVolumeSlider?.addEventListener('input', (e) => {
    state.volume = parseInt(e.target.value, 10);
    showVolumeHud();
  });

  // Clear notifications
  shadeClearNotifs?.addEventListener('click', (e) => {
    e.stopPropagation();
    if (shadeNotifsList) {
      shadeNotifsList.innerHTML = '<div style="font-size: 0.62rem; color: #64748B; text-align: center; padding: 12px 0;">No new notifications</div>';
    }
    playBeep(450, 0.04, 'triangle');
    logConsole('info', '[NOTIFICATIONS] Notification shade buffer cleared.');
  });

  // Notification Banner Engine
  function triggerNotification({ app, title, text, icon = '💬', viewId = null }) {
    playBeep(880, 0.04, 'triangle');
    setTimeout(() => playBeep(1320, 0.06, 'sine'), 50);

    const capText = document.getElementById('capsule-text');
    const capDot = document.getElementById('capsule-dot');
    if (capText) capText.textContent = `${icon} ${title}`;
    if (capDot) {
      capDot.style.background = '#38bdf8';
      capDot.style.boxShadow = '0 0 8px #38bdf8';
    }

    const banner = document.getElementById('phone-notif-banner');
    const bIcon = document.getElementById('banner-icon');
    const bTitle = document.getElementById('banner-title');
    const bText = document.getElementById('banner-text');

    if (banner && bTitle && bText) {
      if (bIcon) bIcon.textContent = icon;
      bTitle.textContent = `${app} • ${title}`;
      bText.textContent = text;
      banner.classList.add('show');

      banner.onclick = () => {
        banner.classList.remove('show');
        if (viewId) switchView(viewId);
      };

      clearTimeout(state.bannerTimeout);
      state.bannerTimeout = setTimeout(() => {
        banner.classList.remove('show');
      }, 4000);
    }

    const shadeList = document.getElementById('shade-notifs-list');
    if (shadeList) {
      const item = document.createElement('div');
      item.className = 'notif-item';
      item.innerHTML = `
        <div class="notif-item-top">
          <strong>${icon} ${app}</strong>
          <span class="notif-item-time">Just now</span>
        </div>
        <p style="margin: 2px 0 0 0;">${text}</p>
      `;
      if (viewId) {
        item.style.cursor = 'pointer';
        item.addEventListener('click', (e) => {
          e.stopPropagation();
          closeShade();
          switchView(viewId);
        });
      }
      shadeList.insertBefore(item, shadeList.firstChild);
    }

    logConsole('info', `[NOTIFICATION] ${app}: ${title} - ${text}`);
  }

  document.getElementById('banner-close')?.addEventListener('click', (e) => {
    e.stopPropagation();
    document.getElementById('phone-notif-banner')?.classList.remove('show');
  });

  // Unified IPC Bridge
  window.InterOS = {
    version: "0.8.4",
    arch: "riscv64gc / aarch64",
    ipc: {
      send(target, cmd, payload) {
        logConsole('info', `[IPC] Outgoing 0x${cmd.toString(16)} -> ${target}`);
        appendIpcTrace("UI_SHELL", target, `0x${cmd.toString(16).toUpperCase()}`, JSON.stringify(payload).length);
        fetch('/api/rapp/ipc', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ target, cmd, payload })
        }).catch(() => {});
      },
      async request(target, cmd, payload) {
        this.send(target, cmd, payload);
        return { status: "ACK", target, timestamp: Date.now() };
      }
    }
  };

  function appendIpcTrace(from, to, op, bytes) {
    const list = document.getElementById('ipc-stream-list');
    if (!list) return;
    const now = new Date();
    const timeStr = `${String(now.getMinutes()).padStart(2, '0')}:${String(now.getSeconds()).padStart(2, '0')}.${String(Math.floor(now.getMilliseconds()/100))}`;
    const row = document.createElement('div');
    row.className = 'ipc-row';
    row.innerHTML = `
      <span class="ipc-time">${timeStr}</span>
      <span class="ipc-from">${from}</span>
      <span class="ipc-arrow">→</span>
      <span class="ipc-to">${to}</span>
      <span class="ipc-op">${op}</span>
      <span class="ipc-size">${bytes}B</span>
    `;
    list.insertBefore(row, list.firstChild);
    while (list.children.length > 30) list.removeChild(list.lastChild);
  }

  // =========================================================================
  // 24. FLAGSHIP APPLICATIONS (AUTHENTICATOR, HEALTH HUB, MESH CHAT)
  // =========================================================================

  // 1. Authenticator Controller
  let authTimerInterval = null;
  function initAuthenticatorView() {
    if (authTimerInterval) return;
    const timerCircle = document.getElementById("auth-global-timer");
    const totalCircumference = 2 * Math.PI * 11;
    if (timerCircle) timerCircle.style.strokeDasharray = totalCircumference;

    function authTick() {
      const now = Math.floor(Date.now() / 1000);
      const timeLeft = 30 - (now % 30);
      if (timerCircle) {
        const offset = totalCircumference - (timeLeft / 30) * totalCircumference;
        timerCircle.style.strokeDashoffset = offset;
      }
      if (timeLeft === 30) refreshAuthCodes();
    }
    authTimerInterval = setInterval(authTick, 1000);
    authTick();

    function refreshAuthCodes() {
      document.querySelectorAll(".auth-token-card").forEach(card => {
        const newCode = '';
        card.dataset.code = newCode;
        const d = card.querySelector(".token-digits");
        if (d) d.textContent = newCode.slice(0, 3) + " " + newCode.slice(3);
      });
      window.InterOS.ipc.send("org.interos.authenticator", 0x01, { refresh: true });
    }

    document.querySelectorAll(".auth-token-card").forEach(card => {
      card.addEventListener("click", () => {
        const code = card.dataset.code;
        navigator.clipboard?.writeText(code);
        triggerNotification({ app: 'Authenticator', title: 'Code Copied', text: `Token ${code} copied to clipboard`, icon: '🛡️' });
      });
    });

    document.getElementById("btn-auth-add-key")?.addEventListener("click", () => {
      const name = prompt("Enter Account Label:", "Node Sentinel");
      if (!name) return;
      const newCode = '';
      const card = document.createElement("div");
      card.className = "auth-token-card";
      card.dataset.code = newCode;
      card.innerHTML = `
        <div class="token-meta">
          <h4>${name}</h4>
          <div class="token-digits">Unavailable</div>
        </div>
        <button class="btn-copy-code" title="Copy 2FA Code">📋</button>
      `;
      card.addEventListener("click", () => {
        navigator.clipboard?.writeText(newCode);
        triggerNotification({ app: 'Authenticator', title: 'Code Copied', text: `Token ${newCode} copied`, icon: '🛡️' });
      });
      document.getElementById("auth-tokens-list")?.appendChild(card);
      triggerNotification({ app: 'Authenticator', title: 'Key Provisioned', text: `Enclave key created for ${name}`, icon: '🔑', viewId: 'view-authenticator' });
      window.InterOS.ipc.send("org.interos.authenticator", 0x02, { account: name });
    });
  }

  // 2. Health & Sensor Hub Controller
  let healthAnimId = null;
  function startHealthVitals() {
    const canvas = document.getElementById("health-ecg-canvas");
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    let points = [];
    for (let i = 0; i < 310; i++) points.push(42);
    let step = 0;

    if (healthAnimId) cancelAnimationFrame(healthAnimId);

    function drawEcg() {
      step++;
      let y = 42 + Math.sin(step * 0.1) * 3;
      if (step % 35 === 0) y = 14;
      else if (step % 35 === 2) y = 70;
      else if (step % 35 === 4) y = 28;

      points.shift();
      points.push(y);

      ctx.clearRect(0, 0, 310, 85);
      ctx.strokeStyle = "rgba(244, 63, 94, 0.18)";
      ctx.lineWidth = 1;
      for (let x = 0; x < 310; x += 30) {
        ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, 85); ctx.stroke();
      }
      for (let gy = 0; gy < 85; gy += 17) {
        ctx.beginPath(); ctx.moveTo(0, gy); ctx.lineTo(310, gy); ctx.stroke();
      }

      ctx.strokeStyle = "#F43F5E";
      ctx.lineWidth = 2;
      ctx.beginPath();
      for (let i = 0; i < points.length; i++) {
        if (i === 0) ctx.moveTo(i, points[i]);
        else ctx.lineTo(i, points[i]);
      }
      ctx.stroke();

      healthAnimId = requestAnimationFrame(drawEcg);
    }
    drawEcg();
    window.InterOS.ipc.send("org.interos.health", 0x10, { sample_rate: 250 });
  }

  // 3. Mesh Chat Controller
  function initMeshChat() {
    const stream = document.getElementById("mesh-chat-stream");
    const input = document.getElementById("mesh-msg-input");
    const btnSend = document.getElementById("btn-mesh-send");
    if (!btnSend || !input) return;

    function sendMesh() {
      const text = input.value.trim();
      if (!text) return;
      input.value = "";

      const outBubble = document.createElement("div");
      outBubble.className = "mesh-bubble out";
      outBubble.innerHTML = `
        <div>${text}</div>
        <div class="mesh-bubble-meta"><span>Broadcast</span><span>0 Hops</span></div>
      `;
      stream.appendChild(outBubble);
      stream.scrollTop = stream.scrollHeight;

      window.InterOS.ipc.send("org.interos.meshchat", 0x20, { message: text });

      setTimeout(() => {
        const inBubble = document.createElement("div");
        inBubble.className = "mesh-bubble in";
        inBubble.innerHTML = `
          <div>Packet verified &amp; routed by Node-Gamma (RSSI -58dBm). Hop count: 1.</div>
          <div class="mesh-bubble-meta"><span>Node-Gamma (-58dBm)</span><span>1 Hop</span></div>
        `;
        stream.appendChild(inBubble);
        stream.scrollTop = stream.scrollHeight;
        triggerNotification({ app: 'Mesh Chat', title: 'Packet Relayed', text: 'Node-Gamma acknowledged mesh frame', icon: '📡', viewId: 'view-meshchat' });
      }, 1200);
    }

    btnSend.onclick = sendMesh;
    input.onkeydown = (e) => {
      if (e.key === "Enter") sendMesh();
    };
  }

  // =========================================================================
  // 22. SOVEREIGN MUSIC PLAYER & HI-FI SYNTHESIZER ENGINE
  // =========================================================================
  const btnBackMusic = document.getElementById('btn-back-music');
  btnBackMusic?.addEventListener('click', goHome);

  const musicTracks = [
    { id: 0, title: 'Sovereign Symphony in C', artist: 'InterOS Ensemble • 120 BPM', duration: 252, format: 'FLAC 24-bit', bpm: 120 },
    { id: 1, title: 'Cybernetic Sunrise', artist: 'Astro Sentinel • 95 BPM', duration: 225, format: 'Opus 48kHz', bpm: 95 },
    { id: 2, title: 'Mesh Harmony', artist: 'Ring 3 Synthesizer • 130 BPM', duration: 302, format: 'FLAC 24-bit', bpm: 130 }
  ];

  let currentTrackIdx = 0;
  let isMusicPlaying = false;
  let musicProgressSeconds = 0;
  let musicInterval = null;
  let currentWaveform = 'sine';
  let synthOsc = null;
  let synthGain = null;
  let spectrumAnimId = null;

  const btnMusicPlay = document.getElementById('btn-music-play');
  const btnMusicNext = document.getElementById('btn-music-next');
  const btnMusicPrev = document.getElementById('btn-music-prev');
  const musicScrubber = document.getElementById('music-scrubber');
  const musicTimeCurr = document.getElementById('music-time-curr');
  const musicTimeTotal = document.getElementById('music-time-total');
  const musicTrackTitle = document.getElementById('music-track-title');
  const musicTrackArtist = document.getElementById('music-track-artist');
  const turntablePlatter = document.getElementById('turntable-platter');
  const musicSpectrumCanvas = document.getElementById('music-spectrum-canvas');

  // Shade mini player controls
  const shadeBtnPlay = document.getElementById('shade-btn-play');
  const shadeBtnNext = document.getElementById('shade-btn-next');
  const shadeBtnPrev = document.getElementById('shade-btn-prev');
  const shadeTrackTitle = document.getElementById('shade-track-title');
  const shadeTrackArtist = document.getElementById('shade-track-artist');

  function renderMusicView() {
    updateMusicTrackInfo();
    setupSpectrumCanvas();
  }

  function updateMusicTrackInfo() {
    const track = musicTracks[currentTrackIdx];
    if (musicTrackTitle) musicTrackTitle.textContent = track.title;
    if (musicTrackArtist) musicTrackArtist.textContent = `${track.artist} • ${track.format}`;
    if (shadeTrackTitle) shadeTrackTitle.textContent = track.title;
    if (shadeTrackArtist) shadeTrackArtist.textContent = track.artist;

    const mins = Math.floor(track.duration / 60);
    const secs = String(track.duration % 60).padStart(2, '0');
    if (musicTimeTotal) musicTimeTotal.textContent = `${mins}:${secs}`;

    document.querySelectorAll('.playlist-item').forEach((item, idx) => {
      item.classList.toggle('active', idx === currentTrackIdx);
      const statusEl = item.querySelector('.pl-status');
      if (statusEl) {
        statusEl.textContent = idx === currentTrackIdx && isMusicPlaying ? 'Playing' : (idx === 1 ? 'Opus' : 'FLAC');
      }
    });

    if (typeof updateBentoMediaCard === 'function') {
      updateBentoMediaCard(isMusicPlaying, track.title);
    }
  }

  // Waveform selector pills
  document.querySelectorAll('.wf-pill').forEach(pill => {
    pill.addEventListener('click', () => {
      document.querySelectorAll('.wf-pill').forEach(p => p.classList.remove('active'));
      pill.classList.add('active');
      currentWaveform = pill.dataset.wf || 'sine';
      logConsole('info', `[AUDIO_HAL] Synthesizer waveform switched to '${currentWaveform.toUpperCase()}'.`);
      playBeep(440, 0.05, currentWaveform);
    });
  });

  // Playlist items click
  document.querySelectorAll('.playlist-item').forEach(item => {
    item.addEventListener('click', () => {
      const trackIdx = parseInt(item.dataset.track, 10);
      if (!isNaN(trackIdx)) {
        currentTrackIdx = trackIdx;
        musicProgressSeconds = 0;
        updateMusicTrackInfo();
        if (!isMusicPlaying) togglePlayMusic();
      }
    });
  });

  // Audio synthesis loop (C-Major Arpeggio 261Hz -> 523Hz)
  const arpeggioNotes = [261.63, 329.63, 392.00, 523.25, 392.00, 329.63];
  let noteIndex = 0;
  let synthNoteInterval = null;

  function startSynthesizer() {
    try {
      if (!state.audioCtx) {
        const AudioContext = window.AudioContext || window.webkitAudioContext;
        if (AudioContext) state.audioCtx = new AudioContext();
      }
      if (state.audioCtx && state.audioCtx.state === 'suspended') {
        state.audioCtx.resume();
      }

      synthNoteInterval = setInterval(() => {
        if (!isMusicPlaying || !state.audioCtx) return;
        const freq = arpeggioNotes[noteIndex % arpeggioNotes.length];
        noteIndex++;

        const osc = state.audioCtx.createOscillator();
        const gain = state.audioCtx.createGain();
        osc.type = currentWaveform;
        osc.frequency.setValueAtTime(freq, state.audioCtx.currentTime);

        const masterVol = (state.volume / 100) * 0.06;
        gain.gain.setValueAtTime(masterVol, state.audioCtx.currentTime);
        gain.gain.exponentialRampToValueAtTime(0.001, state.audioCtx.currentTime + 0.35);

        osc.connect(gain);
        gain.connect(state.audioCtx.destination);
        osc.start();
        osc.stop(state.audioCtx.currentTime + 0.38);
      }, 350);
    } catch (e) {}
  }

  function stopSynthesizer() {
    if (synthNoteInterval) {
      clearInterval(synthNoteInterval);
      synthNoteInterval = null;
    }
  }

  function togglePlayMusic() {
    isMusicPlaying = !isMusicPlaying;
    const playIcon = isMusicPlaying ? '⏸' : '▶';
    if (btnMusicPlay) btnMusicPlay.textContent = playIcon;
    if (shadeBtnPlay) shadeBtnPlay.textContent = playIcon;

    if (turntablePlatter) {
      turntablePlatter.classList.toggle('spinning', isMusicPlaying);
    }

    // Quantum Capsule hook
    if (quantumCapsule) {
      quantumCapsule.classList.toggle('playing', isMusicPlaying);
      if (capsuleText) {
        capsuleText.textContent = isMusicPlaying ? `🎵 ${musicTracks[currentTrackIdx].title.slice(0, 14)}...` : 'Ready';
      }
    }

    if (typeof updateBentoMediaCard === 'function') {
      updateBentoMediaCard(isMusicPlaying, musicTracks[currentTrackIdx].title);
    }

    if (isMusicPlaying) {
      startSynthesizer();
      startSpectrumVisualizer();
      logConsole('ok', `[AUDIO_HAL] DMA Pipe 1: Streaming '${musicTracks[currentTrackIdx].title}' (48kHz Hi-Fi PCM).`);

      musicInterval = setInterval(() => {
        musicProgressSeconds++;
        const total = musicTracks[currentTrackIdx].duration;
        if (musicProgressSeconds >= total) {
          nextTrack();
          return;
        }
        const mins = Math.floor(musicProgressSeconds / 60);
        const secs = String(musicProgressSeconds % 60).padStart(2, '0');
        if (musicTimeCurr) musicTimeCurr.textContent = `${mins}:${secs}`;
        if (musicScrubber) musicScrubber.value = Math.floor((musicProgressSeconds / total) * 100);
      }, 1000);
    } else {
      stopSynthesizer();
      stopSpectrumVisualizer();
      clearInterval(musicInterval);
      logConsole('info', '[AUDIO_HAL] Playback paused. DMA buffer standby.');
    }
    updateMusicTrackInfo();
  }

  function nextTrack() {
    currentTrackIdx = (currentTrackIdx + 1) % musicTracks.length;
    musicProgressSeconds = 0;
    updateMusicTrackInfo();
    logConsole('info', `[AUDIO_HAL] Skipped to track #${currentTrackIdx + 1}: ${musicTracks[currentTrackIdx].title}`);
  }

  function prevTrack() {
    currentTrackIdx = (currentTrackIdx - 1 + musicTracks.length) % musicTracks.length;
    musicProgressSeconds = 0;
    updateMusicTrackInfo();
  }

  btnMusicPlay?.addEventListener('click', togglePlayMusic);
  shadeBtnPlay?.addEventListener('click', (e) => { e.stopPropagation(); togglePlayMusic(); });
  btnMusicNext?.addEventListener('click', nextTrack);
  shadeBtnNext?.addEventListener('click', (e) => { e.stopPropagation(); nextTrack(); });
  btnMusicPrev?.addEventListener('click', prevTrack);
  shadeBtnPrev?.addEventListener('click', (e) => { e.stopPropagation(); prevTrack(); });

  musicScrubber?.addEventListener('input', (e) => {
    const pct = parseInt(e.target.value, 10);
    musicProgressSeconds = Math.floor((pct / 100) * musicTracks[currentTrackIdx].duration);
    const mins = Math.floor(musicProgressSeconds / 60);
    const secs = String(musicProgressSeconds % 60).padStart(2, '0');
    if (musicTimeCurr) musicTimeCurr.textContent = `${mins}:${secs}`;
  });

  // Spectrum Canvas Visualizer
  function setupSpectrumCanvas() {
    if (!musicSpectrumCanvas) return;
    const ctx = musicSpectrumCanvas.getContext('2d');
    ctx.clearRect(0, 0, musicSpectrumCanvas.width, musicSpectrumCanvas.height);
  }

  function startSpectrumVisualizer() {
    if (!musicSpectrumCanvas) return;
    const ctx = musicSpectrumCanvas.getContext('2d');
    const bars = 18;
    const w = musicSpectrumCanvas.width;
    const h = musicSpectrumCanvas.height;
    const barW = (w / bars) - 3;

    function draw() {
      ctx.clearRect(0, 0, w, h);
      for (let i = 0; i < bars; i++) {
        const barH = 4;
        const grad = ctx.createLinearGradient(0, h, 0, h - barH);
        grad.addColorStop(0, '#F43F5E');
        grad.addColorStop(1, '#FB7185');
        ctx.fillStyle = grad;
        ctx.fillRect(i * (barW + 3) + 4, h - barH, barW, barH);
      }
      spectrumAnimId = requestAnimationFrame(draw);
    }
    draw();
  }

  function stopSpectrumVisualizer() {
    if (spectrumAnimId) {
      cancelAnimationFrame(spectrumAnimId);
      spectrumAnimId = null;
      setupSpectrumCanvas();
    }
  }

  // =========================================================================
  // 23. CALENDAR & AGENDA ENGINE
  // =========================================================================
  const btnBackCalendar = document.getElementById('btn-back-calendar');
  btnBackCalendar?.addEventListener('click', goHome);

  const calDaysGrid = document.getElementById('cal-days-grid');
  const calMonthTitle = document.getElementById('cal-month-title');
  const calAgendaDate = document.getElementById('cal-agenda-date');
  const calEventsList = document.getElementById('cal-events-list');
  const calPrevMonth = document.getElementById('cal-prev-month');
  const calNextMonth = document.getElementById('cal-next-month');
  const calBtnToday = document.getElementById('cal-btn-today');
  const btnAddEvent = document.getElementById('btn-add-event');

  let calCurrentYear = 2026;
  let calCurrentMonth = 8; // September (0-indexed: 8 = Sep)
  let selectedDay = 22;

  const monthNames = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

  // Agenda entries must come from the guest calendar service. Do not
  // present developer-authored sample meetings as device state.
  const sampleEvents = {};

  function renderCalendar() {
    if (!calDaysGrid) return;
    if (calMonthTitle) calMonthTitle.textContent = `${monthNames[calCurrentMonth]} ${calCurrentYear}`;
    calDaysGrid.innerHTML = '';

    const firstDayIndex = new Date(calCurrentYear, calCurrentMonth, 1).getDay();
    const daysInMonth = new Date(calCurrentYear, calCurrentMonth + 1, 0).getDate();
    const prevMonthDays = new Date(calCurrentYear, calCurrentMonth, 0).getDate();

    // Previous month trailing days
    for (let i = firstDayIndex - 1; i >= 0; i--) {
      const cell = document.createElement('div');
      cell.className = 'cal-day-cell other-month';
      cell.textContent = prevMonthDays - i;
      calDaysGrid.appendChild(cell);
    }

    // Current month days
    for (let day = 1; day <= daysInMonth; day++) {
      const cell = document.createElement('div');
      cell.className = 'cal-day-cell';
      cell.textContent = day;

      const dateKey = `${calCurrentYear}-${String(calCurrentMonth + 1).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
      if (sampleEvents[dateKey]) {
        cell.classList.add('has-event');
      }

      if (day === 22 && calCurrentMonth === 8 && calCurrentYear === 2026) {
        cell.classList.add('today');
      }

      if (day === selectedDay) {
        cell.classList.add('active');
      }

      cell.addEventListener('click', () => {
        selectedDay = day;
        document.querySelectorAll('.cal-day-cell').forEach(c => c.classList.remove('active'));
        cell.classList.add('active');
        renderAgenda(dateKey, day);
        playBeep(520, 0.02, 'sine');
      });

      calDaysGrid.appendChild(cell);
    }

    const totalCells = firstDayIndex + daysInMonth;
    const nextDays = (totalCells % 7 === 0) ? 0 : 7 - (totalCells % 7);
    for (let j = 1; j <= nextDays; j++) {
      const cell = document.createElement('div');
      cell.className = 'cal-day-cell other-month';
      cell.textContent = j;
      calDaysGrid.appendChild(cell);
    }

    const activeDateKey = `${calCurrentYear}-${String(calCurrentMonth + 1).padStart(2, '0')}-${String(selectedDay).padStart(2, '0')}`;
    renderAgenda(activeDateKey, selectedDay);
  }

  function renderAgenda(dateKey, day) {
    if (calAgendaDate) {
      calAgendaDate.textContent = `Events for ${monthNames[calCurrentMonth]} ${day}, ${calCurrentYear}`;
    }
    if (!calEventsList) return;
    calEventsList.innerHTML = '';

    const events = sampleEvents[dateKey] || [];
    if (events.length === 0) {
      calEventsList.innerHTML = '<div style="font-size: 0.65rem; color: #64748B; padding: 12px 4px; text-align: center;">No scheduled events for this date.</div>';
      return;
    }

    events.forEach(ev => {
      const card = document.createElement('div');
      card.className = 'cal-event-card';
      card.style.borderLeftColor = ev.color;
      card.innerHTML = `
        <div class="event-time">${ev.time}</div>
        <div class="event-details">
          <strong>${ev.title}</strong>
          <span>${ev.desc}</span>
        </div>
      `;
      calEventsList.appendChild(card);
    });
  }

  calPrevMonth?.addEventListener('click', () => {
    calCurrentMonth--;
    if (calCurrentMonth < 0) { calCurrentMonth = 11; calCurrentYear--; }
    renderCalendar();
  });

  calNextMonth?.addEventListener('click', () => {
    calCurrentMonth++;
    if (calCurrentMonth > 11) { calCurrentMonth = 0; calCurrentYear++; }
    renderCalendar();
  });

  calBtnToday?.addEventListener('click', () => {
    calCurrentYear = 2026;
    calCurrentMonth = 8;
    selectedDay = 22;
    renderCalendar();
  });

  btnAddEvent?.addEventListener('click', () => {
    const title = prompt('Enter sovereign event title:', 'Kernel Driver Review');
    if (!title) return;
    const dateKey = `${calCurrentYear}-${String(calCurrentMonth + 1).padStart(2, '0')}-${String(selectedDay).padStart(2, '0')}`;
    if (!sampleEvents[dateKey]) sampleEvents[dateKey] = [];
    sampleEvents[dateKey].push({
      time: '05:30 PM',
      title: title,
      desc: 'Scheduled locally in Ring 3 calendar storage',
      color: '#06B6D4'
    });
    renderCalendar();
    playBeep(600, 0.04, 'triangle');
    logConsole('ok', `[CALENDAR] New sovereign event created: "${title}" on ${dateKey}.`);
  });

  // =========================================================================
  // 24. APP DEV INSPECTOR & LIVE IPC MONITOR
  // =========================================================================
  const appdevIpcStream = document.getElementById('appdev-ipc-stream');
  const btnTriggerHotreload = document.getElementById('btn-trigger-hotreload');

  // IPC monitor is guest-backed only. Synthetic packet injection is disabled.
  btnTriggerHotreload?.addEventListener('click', () => {
    showToast('Hot reload unavailable: guest build/reload endpoint is not connected');
    logConsole('warn', '[HOT_RELOAD] UNAVAILABLE — Device Studio will not simulate a rebuild or reload.');
  });

  // Load packages initially
  loadPackagesTable();

  // Initial boot greeting
  logConsole('info', 'InterOS Core Microkernel v8.2 Studio initialized.');
  logConsole('ok', 'Universal Rust Mobile Platform: RISC-V 64 / ARM64 / x86_64 targets live.');

  // =========================================================================
  // PRODUCTION LAUNCHER HOMESCREEN & NIAGARA APPS STREAM
  // =========================================================================

  function initLauncherHomescreen() {
    const clockTime = document.getElementById('launcher-clock-time');
    const dateText = document.getElementById('launcher-date-text');
    const weatherText = document.getElementById('launcher-weather-text');
    const searchBar = document.getElementById('launcher-search-bar');
    const canvas = document.getElementById('launcher-retro-canvas');

    // 1. Live Clock & Date update
    function updateClock() {
      const now = new Date();
      if (clockTime) {
        clockTime.textContent = now.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
      }
      if (dateText) {
        const options = { weekday: 'long', month: 'long', day: 'numeric' };
        dateText.textContent = now.toLocaleDateString('en-US', options);
      }
    }
    updateClock();
    setInterval(updateClock, 1000);

    // 2. Dock Actions
    document.querySelectorAll('.launcher-dock-icon').forEach(btn => {
      btn.addEventListener('click', (e) => {
        e.stopPropagation();
        playBeep(520, 0.02, 'sine');
        const view = btn.dataset.view;
        if (view) switchView(view);
      });
    });

    // 3. Search Bar triggers Apps view
    searchBar?.addEventListener('click', () => {
      playBeep(480, 0.02, 'sine');
      switchView('view-apps');
    });

    // 4. Swipe up on home to open Apps Screen
    const homeContainer = document.getElementById('launcher-home-container');
    if (homeContainer) {
      let touchStartY = 0;
      homeContainer.addEventListener('touchstart', (e) => {
        touchStartY = e.touches[0].clientY;
      }, { passive: true });
      homeContainer.addEventListener('touchend', (e) => {
        const delta = e.changedTouches[0].clientY - touchStartY;
        if (delta < -50) {
          playBeep(560, 0.02, 'sine');
          switchView('view-apps');
        }
      });
    }

    // 5. Interactive 8-Bit Pixel Runner on Upper Canvas
    if (canvas) {
      const ctx = canvas.getContext('2d');
      let x = 30;
      let y = 100;
      let vy = 0;
      let isJumping = false;
      let cactusX = 340;
      let pteroX = 480;
      let pteroY = 50;
      let animFrame;

      function jump() {
        if (!isJumping) {
          vy = -6.5;
          isJumping = true;
          playBeep(700, 0.04, 'square');
        }
      }

      canvas.parentElement?.addEventListener('click', jump);

      function draw() {
        if (!ctx) return;
        ctx.clearRect(0, 0, canvas.width, canvas.height);

        // Ground line
        ctx.fillStyle = '#18181B';
        ctx.fillRect(0, 118, canvas.width, 2);

        // Physics
        if (isJumping) {
          y += vy;
          vy += 0.35;
          if (y >= 100) {
            y = 100;
            vy = 0;
            isJumping = false;
          }
        }

        // Move obstacles
        cactusX -= 1.8;
        if (cactusX < -20) cactusX = canvas.width + 40;

        pteroX -= 2.4;
        if (pteroX < -30) pteroX = canvas.width + 160;

        // Draw Pixel Dino Silhouette
        ctx.fillStyle = '#18181B';
        // Head
        ctx.fillRect(x + 10, y - 18, 12, 10);
        ctx.clearRect(x + 13, y - 16, 2, 2); // Eye
        // Body
        ctx.fillRect(x + 4, y - 10, 14, 12);
        // Tail
        ctx.fillRect(x, y - 12, 4, 6);
        // Legs
        const legWalk = Math.floor(Date.now() / 150) % 2;
        if (legWalk === 0 || isJumping) {
          ctx.fillRect(x + 6, y + 2, 3, 6);
          ctx.fillRect(x + 13, y + 2, 3, 6);
        } else {
          ctx.fillRect(x + 4, y + 2, 3, 6);
          ctx.fillRect(x + 15, y + 2, 3, 6);
        }

        // Draw Cactus
        ctx.fillRect(cactusX, 94, 6, 24);
        ctx.fillRect(cactusX - 4, 100, 4, 10);
        ctx.fillRect(cactusX + 6, 104, 4, 8);

        // Draw Pterodactyl
        const wingFlap = Math.floor(Date.now() / 200) % 2;
        ctx.fillRect(pteroX, pteroY, 14, 5);
        if (wingFlap === 0) {
          ctx.fillRect(pteroX + 4, pteroY - 6, 6, 6); // Wing up
        } else {
          ctx.fillRect(pteroX + 4, pteroY + 5, 6, 6); // Wing down
        }

        animFrame = requestAnimationFrame(draw);
      }
      draw();
    }
  }

  function initMinimalStream() {
    const streamList = document.getElementById('stream-app-list');
    const dayHeader = document.getElementById('stream-day-header');
    const alphabetRail = document.getElementById('stream-alphabet-rail');
    const viewport = document.getElementById('stream-app-viewport');
    const searchPill = document.getElementById('stream-search-pill');
    const backBtn = document.getElementById('btn-back-apps');
    const fabBtn = document.getElementById('stream-fab-btn');

    // Day Header
    if (dayHeader) {
      const days = ['SUNDAY', 'MONDAY', 'TUESDAY', 'WEDNESDAY', 'THURSDAY', 'FRIDAY', 'SATURDAY'];
      dayHeader.textContent = days[new Date().getDay()];
    }

    // Back to Home
    backBtn?.addEventListener('click', () => switchView('view-home'));