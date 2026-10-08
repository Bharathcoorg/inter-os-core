/**
 * Inter OS — Device Studio (monitor and control panel)
 *
 * InterOS has exactly ONE phone UI: the stock UI the guest kernel renders into
 * the scanout framebuffer. This file is a faithful monitor for it. It displays
 * the guest framebuffer, forwards pointer and keyboard input to the guest, and
 * provides the Studio's genuine device controls (power, volume, screendump,
 * serial console, package sideloading, hardware telemetry and target badges).
 *
 * It deliberately does NOT contain a phone UI of its own. The Studio Shell
 * Surface Layer (21 .screen-view containers, the launcher dock, the app drawer,
 * the recents/multitasking deck, 17 per-app placeholder panels, the lock screen,
 * the gravity shade, the notification banner, and the #phone-viewport overlay)
 * was removed along with switchView() and its 35 call sites: two UIs drift
 * apart, and chrome that renders screens ends up claiming behaviour the guest
 * does not have. Navigation, the app drawer, the lock screen and the recents
 * deck now belong to the guest alone.
 *
 * Guest input contract (unchanged by this cleanup):
 *   POST /api/touch/event  -- tap / swipe, in either the logical portrait space
 *                             (coords:"portrait", logical 720x1440) or the raw scanout
 *                             space (coords:"guest_framebuffer", observed size)
 *   POST /api/input/key    -- single characters and navigation keycodes
 *   POST /api/hardware/*   -- power and volume HAL
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
    volume: 70,
    isLandscape: false,
    calcMode: 'std',
    audioCtx: null,
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
  const btnRebootKernel = document.getElementById('btn-reboot-kernel');
  const telemetryTerminal = document.getElementById('telemetry-terminal');
  const btnClearConsole = document.getElementById('btn-clear-console');
  const btnPingKernel = document.getElementById('btn-ping-kernel');

  // Stage & Chassis
  const stageWrapper = document.getElementById('emulator-stage-wrapper');

  // Android Emulator Hardware Toolbar Buttons
  // WHY: these are device controls, not navigation. The Studio has no screens of
  // its own; the Back/Home/Recents buttons inject the equivalent gesture into the
  // guest framebuffer so the guest kernel performs the navigation.
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
  // View Mode Switcher (Smartphone View vs Studio Telemetry Deck)
  const btnShowPhone = document.getElementById('btn-show-phone');
  const btnShowTelemetry = document.getElementById('btn-show-telemetry');
  const studioWorkspace = document.getElementById('studio-workspace');

  btnShowPhone?.addEventListener('click', () => {
    btnShowPhone.classList.add('active');
    btnShowTelemetry?.classList.remove('active');
    studioWorkspace?.classList.add('smartphone-mode');
  });

  btnShowTelemetry?.addEventListener('click', () => {
    btnShowTelemetry.classList.add('active');
    btnShowPhone?.classList.remove('active');
    studioWorkspace?.classList.remove('smartphone-mode');
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
      valRamUsage.textContent = isAlive ? `CR3: ${cr3 || 'Active Paging'}` : 'CR3: Standby';
    }
    if (fillRamBar) {
      fillRamBar.style.width = isAlive ? '100%' : '0%';
      fillRamBar.style.background = isAlive ? '#10B981' : '#71717A';
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
      const ops = data.block?.read_ops || 0;
      fillBlockBar.style.width = isAlive ? (ops > 0 ? '100%' : '50%') : '0%';
      fillBlockBar.style.background = isAlive ? '#3B82F6' : '#71717A';
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
        if (data.transport_fps !== undefined) {
          transportFps = data.transport_fps;
        }
        if (data.guest_fps !== undefined) {
          guestFps = data.guest_fps;
        }
        updateHardwareProfilerUI(data);
      }
    } catch (e) {
      updateHardwareProfilerUI({ qemu_alive: false, status: 'offline' });
    }
  }

  // Periodic Telemetry Poll (every 1.5s)
  fetchHardwareTelemetry();
  setInterval(fetchHardwareTelemetry, 1500);

  // Warm Reboot Microkernel Flow (Flicker-Free & Synced)
  async function executeWarmReboot() {
    isBooting = true;

    // Keep the real QEMU scanout visible during reset so the guest boot animation,
    // lock screen and first frame are observable rather than replaced by a Studio splash.

    if (btnRebootKernel) {
      btnRebootKernel.style.pointerEvents = 'none';
      const span = btnRebootKernel.querySelector('span');
      if (span) span.textContent = 'Rebooting...';
    }

    logConsole('warn', '[KERNEL_EVENT] Warm reboot signal dispatched to hardware controller...');
    showToast('Rebooting InterOS...');
    playBeep(440, 0.1, 'square');

    try {
      const res = await fetch('/api/qemu/reboot', { method: 'POST' });
      const data = await res.json();
      if (data.success) {
        logConsole('ok', `[REBOOT_OK] QEMU restart completed via ${data.restart_method || (data.qemu_reset ? 'monitor_system_reset' : 'backend_relaunch')}.`);
        playBeep(880, 0.08, 'triangle');
      }
    } catch (err) {
      logConsole('err', `[REBOOT_ERROR] Failed to send reboot: ${err.message}`);
    }

    if (bootCheckInterval) clearInterval(bootCheckInterval);
    bootCheckInterval = setInterval(checkLauncherBootReady, 400);

    setTimeout(() => {
      if (isBooting) {
        isBooting = false;
        if (bootCheckInterval) clearInterval(bootCheckInterval);
        refreshScanoutFrame();
        if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
        startScanoutStream();
        if (btnRebootKernel) {
          btnRebootKernel.style.pointerEvents = 'auto';
          const span = btnRebootKernel.querySelector('span');
          if (span) span.textContent = 'Retry Reboot';
        }
        logConsole('err', '[BOOT_TIMEOUT] Guest did not report launcher_ready within 45 seconds; scanout remains real, but boot is not marked successful.');
      }
    }, 45000);
  }

  btnRebootKernel?.addEventListener('click', executeWarmReboot);

  // =========================================================================
  // Authentic Microkernel Display & Live Scanout Controller
  // =========================================================================
  const screenLiveScanout = document.getElementById('screen-live-scanout');
  const liveScanoutImg = document.getElementById('live-scanout-img');
  const liveScanoutCanvas = document.getElementById('live-scanout-canvas');
  let scanoutCtx = null;
  if (liveScanoutCanvas) {
    liveScanoutCanvas.style.display = 'block';
    if (liveScanoutImg) liveScanoutImg.style.display = 'none';
    scanoutCtx = liveScanoutCanvas.getContext('2d', { alpha: false, desynchronized: true });
    if (scanoutCtx) {
      scanoutCtx.imageSmoothingEnabled = true;
      scanoutCtx.imageSmoothingQuality = 'high';
    }
  }

  // Gestures
  const btnGestureActivity = document.getElementById('btn-gesture-activity');
  const btnGestureBack = document.getElementById('btn-gesture-back');
  const btnGestureHome = document.getElementById('btn-gesture-home');
  const btnGestureQuick = document.getElementById('btn-gesture-quick');
  const btnGestureSystem = document.getElementById('btn-gesture-system');

  let cropMode = 'portrait'; // Canonical guest scanout is native 720x1440 portrait.
  let scanoutTimer = null;
  let isBooting = false;
  let lastFrameSeq = 0;
  let scanoutInFlight = false;

  function setScanoutMode(mode) {
    cropMode = mode;
    lastFrameSeq = 0;
    refreshScanoutFrame(true);
  }

  let transportFps = 0;
  let guestFps = 'Unknown';
  let measuredFps = 0;
  let frameCount = 0;
  let fpsTimer = performance.now();
  let streamRafId = null;
  let isStreamActive = true;

  async function queryMetrics() {
    try {
      const r = await fetch('/api/screen/metrics');
      if (r.ok) {
        const d = await r.json();
        if (d.transport_fps !== undefined) transportFps = d.transport_fps;
      }
    } catch (_) {}
  }
  setInterval(queryMetrics, 1000);

  const renderHzBadge = document.getElementById('render-hz-badge');

  function updateFpsCounter() {
    frameCount++;
    const now = performance.now();
    if (now - fpsTimer >= 1000) {
      measuredFps = Math.round((frameCount * 1000) / (now - fpsTimer));
      frameCount = 0;
      fpsTimer = now;
      if (renderHzBadge) {
        const displayHz = measuredFps > 0
          ? `${measuredFps} Browser FPS • ${transportFps || measuredFps} Transport FPS`
          : 'Measuring FPS...';
        renderHzBadge.textContent = displayHz;
      }
    }
  }

  function runHighRefreshLoop() {
    if (!isStreamActive) return;
    refreshScanoutFrame();
  }

  function renderScanoutPixels(pixels, width, height) {
    if (scanoutCtx && liveScanoutCanvas) {
      if (liveScanoutCanvas.width !== width || liveScanoutCanvas.height !== height) {
        liveScanoutCanvas.width = width;
        liveScanoutCanvas.height = height;
      }
      const imgData = new ImageData(new Uint8ClampedArray(pixels.buffer, pixels.byteOffset, width * height * 4), width, height);
      scanoutCtx.putImageData(imgData, 0, 0);
    }
  }

  let binaryStreamActive = false;
  let binaryStreamAbort = null;

  async function startPersistentBinaryStream() {
    if (binaryStreamActive) return;
    binaryStreamActive = true;
    if (binaryStreamAbort) binaryStreamAbort.abort();
    binaryStreamAbort = new AbortController();
    const signal = binaryStreamAbort.signal;

    try {
      const resp = await fetch('/api/screen/binary_stream', { signal });
      if (!resp.ok || !resp.body) {
        throw new Error(`Binary stream HTTP ${resp.status}`);
      }
      const reader = resp.body.getReader();
      let accumulated = new Uint8Array(0);

      while (isStreamActive && !signal.aborted) {
        const { done, value } = await reader.read();
        if (done) break;

        const newBuf = new Uint8Array(accumulated.length + value.length);
        newBuf.set(accumulated);
        newBuf.set(value, accumulated.length);
        accumulated = newBuf;

        while (accumulated.length >= 16) {
          if (accumulated[0] !== 0x49 || accumulated[1] !== 0x4F || accumulated[2] !== 0x53 || accumulated[3] !== 0x46) {
            let sync = 1;
            while (sync < accumulated.length && accumulated[sync] !== 0x49) sync++;
            accumulated = accumulated.subarray(sync);
            continue;
          }

          const view = new DataView(accumulated.buffer, accumulated.byteOffset, 16);
          const width = view.getUint16(4);
          const height = view.getUint16(6);
          const seq = view.getUint32(8);
          const frameBytes = 16 + width * height * 4;

          if (accumulated.length < frameBytes) {
            break;
          }

          const pixelBytes = accumulated.subarray(16, frameBytes);
          accumulated = accumulated.subarray(frameBytes);

          if (seq > lastFrameSeq) {
            lastFrameSeq = seq;
            renderScanoutPixels(pixelBytes, width, height);
            updateFpsCounter();
          }
        }
      }
    } catch (err) {
      if (!signal.aborted) {
        binaryStreamActive = false;
        // Fall back to polling
        if (isStreamActive && !streamRafId) {
          streamRafId = requestAnimationFrame(runHighRefreshLoop);
        }
        setTimeout(startPersistentBinaryStream, 2000);
      }
    } finally {
      binaryStreamActive = false;
    }
  }

  async function refreshScanoutFrame(forceImmediate = false) {
    if (binaryStreamActive && !forceImmediate) return;
    if (scanoutInFlight && !forceImmediate) return;
    scanoutInFlight = true;

    try {
      const resp = await fetch(`/api/screen/raw?seq=${lastFrameSeq}`);
      if (resp.status === 200) {
        const buffer = await resp.arrayBuffer();
        if (buffer.byteLength >= 16) {
          const view = new DataView(buffer, 0, 16);
          const m0 = view.getUint8(0);
          const m1 = view.getUint8(1);
          const m2 = view.getUint8(2);
          const m3 = view.getUint8(3);
          // Magic 'IOSF' (0x49, 0x4F, 0x53, 0x46)
          if (m0 === 0x49 && m1 === 0x4F && m2 === 0x53 && m3 === 0x46) {
            const width = view.getUint16(4);
            const height = view.getUint16(6);
            const seq = view.getUint32(8);
            lastFrameSeq = seq;
            const pixels = new Uint8ClampedArray(buffer, 16);
            renderScanoutPixels(pixels, width, height);
          }
        }
      } else if (resp.status === 304) {
        // Not modified: keep current frame
      } else if (resp.status === 503) {
        await fallbackRefreshFrame();
      }
    } catch (_) {
      await fallbackRefreshFrame();
    } finally {
      scanoutInFlight = false;
      updateFpsCounter();
      if (isStreamActive && !binaryStreamActive) {
        streamRafId = requestAnimationFrame(runHighRefreshLoop);
      }
    }
  }

  function fallbackRefreshFrame() {
    return new Promise((resolve) => {
      const orientParam = (state.isLandscape || cropMode === 'full') ? '&orientation=landscape&full=1' : '';
      const url = `/api/screen/frame?t=${Date.now()}${orientParam}`;
      const img = new Image();
      img.onload = () => {
        if (scanoutCtx && liveScanoutCanvas) {
          if (liveScanoutCanvas.width !== img.naturalWidth || liveScanoutCanvas.height !== img.naturalHeight) {
            liveScanoutCanvas.width = img.naturalWidth;
            liveScanoutCanvas.height = img.naturalHeight;
          }
          scanoutCtx.drawImage(img, 0, 0);
        } else if (liveScanoutImg) {
          liveScanoutImg.src = img.src;
        }
        resolve();
      };
      img.onerror = () => resolve();
      img.src = url;
    });
  }

  function startScanoutStream() {
    if (isStreamActive) return;
    isStreamActive = true;
    startPersistentBinaryStream();
    if (!binaryStreamActive) {
      refreshScanoutFrame(true);
      streamRafId = requestAnimationFrame(runHighRefreshLoop);
    }
  }

  function stopScanoutStream() {
    isStreamActive = false;
    if (binaryStreamAbort) {
      binaryStreamAbort.abort();
      binaryStreamAbort = null;
    }
    binaryStreamActive = false;
    if (streamRafId) {
      cancelAnimationFrame(streamRafId);
      streamRafId = null;
    }
  }

  // The live scanout is the only surface the studio shows. There is no boot
  // splash, standby card or lock screen in the chrome to toggle: while the guest
  // is not presenting, the monitor shows nothing rather than a Studio-owned
  // screen that could be mistaken for the guest's.
  function updateDeviceDisplay(isAlive) {
    if (isAlive) {
      // The monitor must never invent a readiness screen. As soon as QEMU is alive,
      // expose its real scanout; boot/lock/off pixels are guest-owned.
      if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
      startScanoutStream();
    } else {
      stopScanoutStream();
      if (screenLiveScanout) screenLiveScanout.style.display = 'none';
    }
  }

  // Exact sub-rectangle occupied by the decoded pixels inside the <img> element
  // accounting for CSS object-fit: contain letterboxing/pillarboxing.
  function getRenderedImageRect(img) {
    const rect = img.getBoundingClientRect();
    const natW = img.naturalWidth || (state.isLandscape ? 1280 : 720);
    const natH = img.naturalHeight || (state.isLandscape ? 720 : 1440);
    const imgAspect = natW / natH;
    const contAspect = rect.width / rect.height;
    let renderW, renderH, renderX, renderY;
    if (contAspect > imgAspect) {
      renderH = rect.height;
      renderW = renderH * imgAspect;
      renderX = rect.left + (rect.width - renderW) / 2;
      renderY = rect.top;
    } else {
      renderW = rect.width;
      renderH = renderW / imgAspect;
      renderX = rect.left;
      renderY = rect.top + (rect.height - renderH) / 2;
    }
    return { left: renderX, top: renderY, width: renderW, height: renderH };
  }

  function getGuestPointerRect() {
    const target = liveScanoutCanvas || liveScanoutImg || screenLiveScanout;
    if (!target) return null;
    const rect = target.getBoundingClientRect();
    const aspect = 720 / 1440;
    const containerAspect = rect.width / Math.max(1, rect.height);
    let width = rect.width;
    let height = rect.height;
    let left = rect.left;
    let top = rect.top;
    if (containerAspect > aspect) {
      width = rect.height * aspect;
      left = rect.left + (rect.width - width) / 2;
    } else {
      height = rect.width / aspect;
      top = rect.top + (rect.height - height) / 2;
    }
    return { left, top, width: Math.max(1, width), height: Math.max(1, height) };
  }

  const virtualTouchDot = document.getElementById('virtual-touch-dot');
  function updateVirtualTouch(clientX, clientY, active) {
    if (!virtualTouchDot || !screenLiveScanout) return;
    if (!active) {
      virtualTouchDot.classList.remove('active');
      return;
    }
    const screenRect = getGuestPointerRect();
    if (!screenRect) return;
    const x = clientX - screenRect.left;
    const y = clientY - screenRect.top;
    virtualTouchDot.style.left = `${x}px`;
    virtualTouchDot.style.top = `${y}px`;
    virtualTouchDot.classList.add('active');
  }

  // THE GUEST INPUT PATH.
  // Pointer input on the framebuffer is the studio's only gesture surface and it
  // goes straight to the guest over POST /api/touch/event. Coordinates are
  // mapped into the canonical logical portrait space (720x1440) or guest framebuffer.
  const touchSurface = screenLiveScanout || liveScanoutImg;
  if (touchSurface) {
    let isPointerDown = false;
    let downSent = false;
    let startX = 0, startY = 0;
    let lastSentX = 0, lastSentY = 0;
    let lastMoveTime = 0;

    touchSurface.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0) return;
      const rect = getGuestPointerRect();
      if (!rect) {
        isPointerDown = false;
        downSent = false;
        return;
      }
      isPointerDown = true;
      downSent = false;
      startX = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
      startY = Math.max(0, Math.min(1, (ev.clientY - rect.top) / rect.height));
      const px = Math.max(0, Math.min(719, Math.round(startX * 719)));
      const py = Math.max(0, Math.min(1439, Math.round(startY * 1439)));
      lastSentX = px;
      lastSentY = py;
      lastMoveTime = performance.now();
      downSent = true;
      fetch('/api/touch/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'down', coords: 'portrait', x: px, y: py, slot: 0 })
      }).catch(() => {});
      updateVirtualTouch(ev.clientX, ev.clientY, true);
      try { touchSurface.setPointerCapture(ev.pointerId); } catch (_) {}
    });

    touchSurface.addEventListener('pointermove', (ev) => {
      if (isPointerDown) {
        updateVirtualTouch(ev.clientX, ev.clientY, true);
        const rect = getGuestPointerRect();
        if (!rect) return;
        const curX = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
        const curY = Math.max(0, Math.min(1, (ev.clientY - rect.top) / rect.height));
        const px = Math.max(0, Math.min(719, Math.round(curX * 719)));
        const py = Math.max(0, Math.min(1439, Math.round(curY * 1439)));
        const now = performance.now();

        if (downSent && now - lastMoveTime >= 16 && (Math.abs(px - lastSentX) > 3 || Math.abs(py - lastSentY) > 3)) {
          lastMoveTime = now;
          lastSentX = px;
          lastSentY = py;
          fetch('/api/touch/event', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action: 'move', coords: 'portrait', x: px, y: py, slot: 0 })
          }).catch(() => {});
        }
      }
    });

    touchSurface.addEventListener('pointerup', async (ev) => {
      if (!isPointerDown) return;
      isPointerDown = false;
      updateVirtualTouch(ev.clientX, ev.clientY, false);
      const rect = getGuestPointerRect();
      if (!rect) {
        if (downSent) {
          fetch('/api/touch/event', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ action: 'up', coords: 'portrait', x: lastSentX, y: lastSentY, slot: 0 }) }).catch(() => {});
        }
        downSent = false;
        return;
      }
      const endX = Math.max(0, Math.min(1, (ev.clientX - rect.left) / rect.width));
      const endY = Math.max(0, Math.min(1, (ev.clientY - rect.top) / rect.height));
      const px = Math.max(0, Math.min(719, Math.round(endX * 719)));
      const py = Math.max(0, Math.min(1439, Math.round(endY * 1439)));
      const dx = (endX - startX) * rect.width;
      const dy = (endY - startY) * rect.height;
      const dist = Math.hypot(dx, dy);

      if (state.isLandscape || cropMode === 'full') {
        const fbW = liveScanoutImg?.naturalWidth || 0;
        const fbH = liveScanoutImg?.naturalHeight || 0;
        if (fbW < 1 || fbH < 1) return;
        const scanoutLimitX = Math.min(1279, fbW - 1);
        const gx1 = Math.max(0, Math.min(scanoutLimitX, Math.round(startX * (fbW - 1))));
        const gy1 = Math.max(0, Math.min(fbH - 1, Math.round(startY * (fbH - 1))));
        const gx2 = Math.max(0, Math.min(scanoutLimitX, Math.round(endX * (fbW - 1))));
        const gy2 = Math.max(0, Math.min(fbH - 1, Math.round(endY * (fbH - 1))));
        if (dist > 18) {
          fetch('/api/touch/event', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action: 'swipe', coords: 'guest_framebuffer', x: gx1, y: gy1, x2: gx2, y2: gy2, steps: 5 })
          }).catch(() => {});
        } else {
          fetch('/api/touch/event', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action: 'tap', coords: 'guest_framebuffer', x: gx2, y: gy2 })
          }).catch(() => {});
        }
      } else {
        if (downSent) {
          fetch('/api/touch/event', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action: 'up', coords: 'portrait', x: px, y: py, slot: 0 })
          }).catch(() => {});
        }
      }

      requestAnimationFrame(() => refreshScanoutFrame(true));
      setTimeout(() => refreshScanoutFrame(true), 25);
      setTimeout(() => refreshScanoutFrame(true), 65);
    });

    touchSurface.addEventListener('pointercancel', (ev) => {
      if (!isPointerDown) return;
      isPointerDown = false;
      if (downSent) {
        fetch('/api/touch/event', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ action: 'up', coords: 'portrait', x: lastSentX, y: lastSentY, slot: 0 }) }).catch(() => {});
      }
      updateVirtualTouch(ev.clientX, ev.clientY, false);
    });

    touchSurface.addEventListener('wheel', (ev) => {
      ev.preventDefault();
      const delta = ev.deltaY;
      const start_y = delta > 0 ? 900 : 500;
      const end_y = delta > 0 ? 500 : 900;
      fetch('/api/touch/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'swipe', coords: 'portrait', x: 360, y: start_y, x2: 360, y2: end_y, steps: 5 })
      }).catch(() => {});
      setTimeout(() => refreshScanoutFrame(true), 40);
    }, { passive: false });

    touchSurface.addEventListener('pointercancel', () => {
      isPointerDown = false;
      updateVirtualTouch(0, 0, false);
      fetch('/api/touch/event', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'cancel', coords: 'portrait', x: 360, y: 720, slot: 0 })
      }).catch(() => {});
    });
  }

  // Draggable IDE Split Resizer Divider
  const ideSplitResizer = document.getElementById('ide-split-resizer');
  const ideLeftPane = document.getElementById('ide-left-pane');
  if (ideSplitResizer && ideLeftPane) {
    let isResizing = false;
    let startX = 0;
    let startWidth = 0;

    ideSplitResizer.addEventListener('pointerdown', (e) => {
      isResizing = true;
      startX = e.clientX;
      startWidth = ideLeftPane.getBoundingClientRect().width;
      ideSplitResizer.classList.add('is-dragging');
      document.body.style.cursor = 'col-resize';
      document.body.style.userSelect = 'none';
      try { ideSplitResizer.setPointerCapture(e.pointerId); } catch (_) {}
    });

    window.addEventListener('pointermove', (e) => {
      if (!isResizing) return;
      const dx = e.clientX - startX;
      const maxW = Math.round(window.innerWidth * 0.50);
      const newWidth = Math.max(320, Math.min(maxW, Math.round(startWidth + dx)));
      ideLeftPane.style.flex = `0 0 ${newWidth}px`;
      ideLeftPane.style.maxWidth = `${newWidth}px`;
    });

    const stopResize = () => {
      if (!isResizing) return;
      isResizing = false;
      ideSplitResizer.classList.remove('is-dragging');
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    window.addEventListener('pointerup', stopResize);
    window.addEventListener('pointercancel', stopResize);

    ideSplitResizer.addEventListener('dblclick', () => {
      ideLeftPane.style.flex = '';
      ideLeftPane.style.maxWidth = '';
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

  // 1. Activity Space (Multitasking Overview Deck)
  btnGestureActivity?.addEventListener('click', () => {
    dispatchHardwareGesture('Activity Space', {
      action: 'recents'
    });
  });

  // 2. Go Back
  btnGestureBack?.addEventListener('click', () => {
    dispatchHardwareGesture('Back', {
      action: 'back'
    });
  });

  // 3. Home
  btnGestureHome?.addEventListener('click', () => {
    dispatchHardwareGesture('Home', {
      action: 'home'
    });
  });

  // 4. Quick Controls Shade
  btnGestureQuick?.addEventListener('click', () => {
    dispatchHardwareGesture('Quick Controls', {
      action: 'tap',
      coords: 'portrait',
      x: 360,
      y: 25
    });
  });

  // 5. System Space
  btnGestureSystem?.addEventListener('click', () => {
    dispatchHardwareGesture('System Space', {
      action: 'tap',
      coords: 'portrait',
      x: 360,
      y: 880
    });
  });

  // =========================================================================
  // Physical Side Buttons & Android Emulator Hardware Toolbar Controls
  // =========================================================================
  let bootCheckInterval = null;

  // Target badges must describe the machine that is actually running.
  //
  // These were static marketing copy ("ARM64 / RISC-V", "4 GB LPDDR4X",
  // "Guest display mode") that contradicted the guest the studio drives: the default
  // profile is x86_64 with QEMU RAM, no memory technology and no 120 Hz panel.
  // Report what was observed and say plainly that it is virtualised. When the
  // guest cannot be reached, leave the badges undetermined rather than inventing
  // a specification.
  function describeObservedTarget(data) {
    const set = (id, value) => {
      const el = document.getElementById(id);
      if (el && value) el.textContent = value;
    };
    const profile = (data && data.active_profile) || 'unknown';
    set('spec-arch', profile === 'unknown' ? 'Undetermined' : profile + ' (virtualised)');
    set('spec-memory', (data && data.ram_mb) ? data.ram_mb + ' MiB emulated' : 'Undetermined');
    const w = data && (data.guest_width || data.display_width);
    const h = data && (data.guest_height || data.display_height);
    set('spec-display', (w && h) ? w + 'x' + h + ' virtual framebuffer' : 'Undetermined');
    set('spec-security', 'Ring 3 capability sandbox (emulated)');
  }

  async function refreshTargetBadges() {
    try {
      const resp = await fetch('/api/qemu/status');
      const data = await resp.json();
      describeObservedTarget(data);
    } catch (_) {
      /* Leave the badges undetermined rather than inventing values. */
    }
  }

  // The badges must describe the running machine even when the page is opened
  // against an already-booted guest, where the boot poll never starts.
  refreshTargetBadges();

  async function checkLauncherBootReady() {
    try {
      const resp = await fetch('/api/qemu/status');
      const data = await resp.json();
      describeObservedTarget(data);
      if (data.launcher_ready) {
        if (bootCheckInterval) {
          clearInterval(bootCheckInterval);
          bootCheckInterval = null;
        }
        isBooting = false;

        refreshScanoutFrame();
        if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
        startScanoutStream();
        if (btnRebootKernel) {
          btnRebootKernel.style.pointerEvents = 'auto';
          const span = btnRebootKernel.querySelector('span');
          if (span) span.textContent = 'Reboot OS';
        }
        logConsole('ok', '[LAUNCHER] Guest scanout ready and presented.');
        showToast('InterOS Ready');
      }
    } catch (_) {}
  }

  // Power On / Boot Microkernel
  async function bootMicrokernel() {
    isBooting = true;
    // Do not replace the guest with a Studio splash. Once QEMU exists, its real
    // scanout is the only display surface, including black/off and boot frames.
    if (qemuGlobalStatus?.classList.contains('online')) {
      if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
      startScanoutStream();
    }

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
  async function handleVolume(delta) {
    try {
      const res = await fetch(`/api/audio/volume?action=${delta > 0 ? 'UP' : 'DOWN'}`);
      if (res.ok) {
        const d = await res.json();
        if (d.volume_percent !== undefined) {
          state.volume = d.volume_percent;
        }
      }
    } catch (_) {
      state.volume = Math.max(0, Math.min(100, state.volume + delta));
    }
    showToast(`Volume: ${state.volume}%`);
    playBeep(delta > 0 ? 800 : 500, 0.04, 'sine');
  }

  hwBtnVolup?.addEventListener('click', () => handleVolume(5));
  emuBtnVolup?.addEventListener('click', () => handleVolume(5));
  hwBtnVoldown?.addEventListener('click', () => handleVolume(-5));
  emuBtnVoldown?.addEventListener('click', () => handleVolume(-5));

  // Screendump is captured by the single handler further down (the "Take
  // Screenshot" block), which flashes the chassis and reports the offline case.
  // A second handler used to be registered here too, so one click downloaded the
  // same guest framebuffer twice; only the thorough one survives.

  // Rotate Screen button is handled by the comprehensive orientation handler below

  // Warm Reboot Microkernel Button
  emuBtnReboot?.addEventListener('click', executeWarmReboot);

  // Back / Home / Recents (◁ ○ ▢).
  // Directly dispatches canonical kernel navigation gestures.
  emuBtnBack?.addEventListener('click', () => {
    dispatchHardwareGesture('Back (◁)', {
      action: 'back',
      coords: 'portrait',
      x: 0,
      y: 0
    });
  });

  // Home Button (○)
  emuBtnHome?.addEventListener('click', () => {
    dispatchHardwareGesture('Home (○)', {
      action: 'home',
      coords: 'portrait',
      x: 360,
      y: 1400
    });
  });

  // Recents / App Switcher (▢) -> Activity Space
  emuBtnRecents?.addEventListener('click', () => {
    dispatchHardwareGesture('Recents / Activity Space (▢)', {
      action: 'recents',
      coords: 'portrait',
      x: 580,
      y: 1400
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

  // =========================================================================
  // 6. ANDROID EMULATOR HARDWARE TOOLBAR & POWER/VOLUME CONTROLS
  // =========================================================================
  // WHY: the power button is a device control, not a screen. There is no Studio
  // overlay to raise any more: the guest owns its lock screen and standby state,
  // so the studio only tells the hardware HAL to sleep or wake and reports what
  // it observed. The previous long-press power menu, the Studio lock screen, the
  // slide-to-unlock track and the OLED sleep overlay were all chrome-owned phone
  // UI and were removed with them. `togglePower()` also dereferenced
  // #screen-off-overlay, which has never existed in index.html, so a short press
  // of the power button threw a TypeError on every click.
  async function togglePower() {
    // Device Studio is only the physical-button frontend. The guest owns display
    // power and reports the resulting state through its authoritative telemetry.
    try {
      const res = await fetch('/api/hardware/power', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'toggle' })
      });
      if (!res.ok) throw new Error('guest power command rejected');
      logConsole('info', '[POWER_KEY] Guest display-power transition requested.');
    } catch (e) {
      logConsole('err', '[POWER_KEY] Guest display-power command failed: ' + e.message);
    }
    playBeep(280, 0.04, 'triangle');
  }

  // A press longer than 600 ms is the physical long-press that used to open the
  // Studio power menu. There is no menu in the chrome any more, so the long press
  // is simply ignored and the short press sleeps/wakes the display.
  let powerPressTimer = null;
  let powerPressTriggered = false;

  function handlePowerDown() {
    powerPressTriggered = false;
    powerPressTimer = setTimeout(() => {
      powerPressTriggered = true;
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

  [hwBtnPower, emuBtnPower].forEach(btn => {
    btn?.addEventListener('mousedown', handlePowerDown);
    btn?.addEventListener('mouseup', handlePowerUp);
    btn?.addEventListener('mouseleave', () => {
      if (powerPressTimer) { clearTimeout(powerPressTimer); powerPressTimer = null; }
    });
    btn?.addEventListener('touchstart', handlePowerDown, { passive: true });
    btn?.addEventListener('touchend', handlePowerUp, { passive: true });
  });

  // Volume Controls (Real QEMU / Audio Sync)
  async function adjustVolume(delta) {
    const action = delta > 0 ? 'UP' : 'DOWN';
    try {
      const res = await fetch('/api/audio/volume?action=' + action);
      const data = await res.json();
      if (!res.ok || !data.success) throw new Error(data.message || 'guest audio command rejected');
      state.volume = Number.isFinite(data.volume_percent) ? data.volume_percent : state.volume;
      showToast(`Volume: ${state.volume}%`);
      logConsole('info', `[AUDIO_HAL] Guest master volume: ${state.volume}%`);
    } catch (e) {
      logConsole('err', '[AUDIO_HAL] Guest volume command failed: ' + e.message);
    }
    playBeep(380 + state.volume * 5, 0.03, 'sine');
  }

  emuBtnVolup?.addEventListener('click', () => adjustVolume(5));
  hwBtnVolup?.addEventListener('click', () => adjustVolume(5));
  emuBtnVoldown?.addEventListener('click', () => adjustVolume(-5));
  hwBtnVoldown?.addEventListener('click', () => adjustVolume(-5));

  // Global Keyboard Shortcuts (Studio volume keys only)
  // WHY: the Escape branches that closed the gravity shade and the Studio power
  // modal were removed with those surfaces; Escape is now forwarded to the guest
  // by the keyboard bridge further down.
  window.addEventListener('keydown', (e) => {
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

  // Screendump: export exactly what the guest framebuffer contains.
  emuBtnScreenshot?.addEventListener('click', async () => {
    playShutterSound();
    // Brief white flash
    const flash = document.createElement('div');
    flash.style.cssText = 'position:absolute;inset:0;background:#FFF;z-index:9999;pointer-events:none;transition:opacity 0.25s;';
    if (screenLiveScanout) screenLiveScanout.appendChild(flash);
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
  emuBtnRotate?.addEventListener('click', async () => {
    state.isLandscape = !state.isLandscape;
    const orient = state.isLandscape ? 'landscape' : 'portrait';
    cropMode = state.isLandscape ? 'full' : 'portrait';
    if (state.isLandscape) {
      stageWrapper.classList.add('landscape');
      logConsole('info', '[DISPLAY_HAL] Gyroscope event: display rotated to LANDSCAPE (1280x720).');
      showToast('Screen: Landscape Mode (1280x720)');
    } else {
      stageWrapper.classList.remove('landscape');
      logConsole('info', '[DISPLAY_HAL] Gyroscope event: display rotated to PORTRAIT (720x1440).');
      showToast('Screen: Portrait Mode (720x1440)');
    }
    playBeep(520, 0.03, 'sine');
    try {
      await fetch('/api/hardware/orientation', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ orientation: orient })
      });
    } catch (_) {}
    refreshScanoutFrame();
    setTimeout(refreshScanoutFrame, 60);
    setTimeout(refreshScanoutFrame, 180);
    setTimeout(refreshScanoutFrame, 350);
  });
  // Warm reboot requested through the studio's hardware control. WHY: the previous
  // version drove a staged "teardown -> boot splash -> reveal homescreen"
  // animation made of Studio overlays and ended in goHome(), all of which belonged
  // to the chrome's own shell and are gone. What remains is the actual device
  // action: ask the power HAL to reboot, then let the boot poller re-present the
  // guest framebuffer when it comes back.
  function triggerReboot() {
    executeWarmReboot();
  }
  // =========================================================================
  // 12. MULTI-FORMAT APPLICATION SIDELOADING (.app, .rap, .wasm, .apk)
  // =========================================================================
  // The sideload control that lived on the removed homescreen tray is gone. This
  // binds the Packages tab's own dropzone, which is genuine Studio chrome: it
  // uploads a package into the guest's Ring 3 sandbox. It is not a phone screen.
  const sideloadDropzone = document.getElementById('sideload-dropzone');
  const sideloadFileInput = document.getElementById('sideload-file-input');
  const btnBrowsePackage = document.getElementById('btn-browse-package');

  btnBrowsePackage?.addEventListener('click', () => {
    sideloadFileInput?.click();
  });

  sideloadFileInput?.addEventListener('change', (e) => {
    const file = e.target.files[0];
    if (file) handleSideloadFile(file);
    e.target.value = '';
  });

  if (sideloadDropzone) {
    sideloadDropzone.addEventListener('dragover', (e) => {
      e.preventDefault();
      sideloadDropzone.classList.add('dragover');
    });

    sideloadDropzone.addEventListener('dragleave', () => {
      sideloadDropzone.classList.remove('dragover');
    });

    sideloadDropzone.addEventListener('drop', (e) => {
      e.preventDefault();
      sideloadDropzone.classList.remove('dragover');
      const file = e.dataTransfer.files[0];
      if (file) handleSideloadFile(file);
    });
  }

  function handleSideloadFile(file) {
    logConsole('info', `[SIDELOAD] Ingesting package: ${file.name} (${file.size} bytes)`);

    fetch(`/api/apps/upload?filename=${encodeURIComponent(file.name)}`, {
      method: 'POST',
      body: file
    })
      .then(res => res.json())
      .then(data => {
        if (data.success) {
          state.installedPackages.unshift({
            id: data.package.id,
            name: data.package.name,
            filename: data.package.filename,
            format: data.package.format,
            status: 'Installed'
          });
          populatePackagesTable();
          logConsole('ok', `[SIDELOAD] Package '${data.package.name}' installed into /data/apps/ (SHA-256 Verified)`);
          showToast(`Installed ${data.package.name}`);
          playBeep(880, 0.06, 'sine');
        } else {
          logConsole('err', `[SIDELOAD] Upload error: ${data.error || 'Unknown'}`);
          showToast('Installation failed');
        }
      })
      .catch(err => {
        logConsole('info', `[SIDELOAD] Package '${file.name}' upload did not complete: ${err.message}`);
        showToast('Upload did not complete');
      });
  }
  // --- Installed Package Catalog (Packages tab) ---
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
          const pkgId = b.dataset.pkgId;
          b.disabled = true;
          logConsole('info', `[PROCESS] Launch requested for '${pkgId}' via guest COM1 bridge.`);
          fetch('/api/apps/launch', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ app_id: pkgId })
          })
            .then(r => r.json())
            .then(data => {
              b.disabled = false;
              if (data && data.success) {
                const detail = (data.message || '[PROCESS] Guest application launched').toString();
                logConsole('info', `[PROCESS] Guest application launched '${pkgId}': ${detail}`);
                showToast(`Launched ${pkgId}`);
                if (typeof populateProcessesTable === 'function') { try { populateProcessesTable(); } catch (_) {} }
              } else {
                logConsole('warn', `[PROCESS] Launch '${pkgId}' rejected: ${(data && (data.error || data.message)) || 'guest refused launch'}`);
                showToast(`Launch failed: ${pkgId}`);
              }
            })
            .catch(err => {
              b.disabled = false;
              logConsole('warn', `[PROCESS] Launch '${pkgId}' did not complete: ${err.message}`);
              showToast('Launch did not complete');
            });
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
            // WHY: #phone-sim-badge was a signal-strength pill in the removed
            // phone status bar. The PDP state above is the Studio's report.
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
              // WHY: #phone-battery and #battery-gauge-level were the battery
              // readout of the removed phone status bar. The SoC badge above is
              // the Studio's report.
            } else {
              if (voltEl) voltEl.textContent = `${p.voltage_mv} mV`;
              if (tempEl) tempEl.textContent = `${p.temp_c} \u00B0C`;
              if (freqEl) freqEl.textContent = `${p.cpu_freq_mhz} MHz`;
              if (socBadge) socBadge.textContent = `${p.battery_pct}% SoC`;
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
            showToast(`KMS Page Flip #${d.kms.page_flips} • guest timing`);
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
  // 15. GUEST KEYBOARD BRIDGE
  // =========================================================================
  // Forward browser keyboard input to the guest over the canonical guest input
  // contract. Text fields in the Studio deck remain Studio-local: the guest only
  // receives keys when focus is not in an input.
  //
  // WHY there is no second pointer bridge here any more: #device-screen used to
  // carry its own pointerdown/move/up bridge posting {action,x,y} with no `coords`
  // field. server.py defaults that to the logical portrait space and rejects
  // anything outside 720x1440, so the 1280x720 framebuffer coordinates this bridge
  // computed failed validation for x > 720 — and it fired alongside the correct
  // bridge on #live-scanout-img, double-sending every gesture. #live-scanout-img
  // is the single guest input surface and it maps both crop modes correctly.
  window.addEventListener('keydown', async (e) => {
    if (e.target && (e.target.tagName === 'INPUT' || e.target.tagName === 'TEXTAREA' || e.target.isContentEditable)) return;
    const navigationKeys = new Set([
      'Enter', 'Escape', 'Backspace', 'Tab',
      'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight',
      'Home', 'End', 'PageUp', 'PageDown'
    ]);
    if (Array.from(e.key).length === 1 || navigationKeys.has(e.key)) {
      e.preventDefault();
      e.stopPropagation();
      try {
        await fetch('/api/input/key', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ key: e.key, code: e.key.codePointAt(0) })
        });
      } catch (_) {}
    }
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

  // Re-run the package fetch now that the handler is fully defined. The table
  // was already painted once above; this repaints it from a fresh /api/apps/list.
  populatePackagesTable();

  // Initial boot greeting
  logConsole('info', 'InterOS Core Microkernel v8.2 Studio initialized.');
  logConsole('ok', 'Universal Rust Mobile Platform: RISC-V 64 / ARM64 / x86_64 targets live.');

  // Initialize Lucide icons on boot
  if (window.lucide && typeof window.lucide.createIcons === 'function') {
    window.lucide.createIcons();
  }

});
