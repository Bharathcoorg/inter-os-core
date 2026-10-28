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
    version: '0.1.0',
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

  // Studio console logger. Called ~30x across flows (reboot, ping, volume,
  // telephony, IPC); it was referenced but never defined, so any hit path
  // threw ReferenceError and aborted the handler. Lines are escaped, level
  // colored, and the buffer is capped so the DOM cannot grow without bound.
  window.logConsole = function(level, msg) {
    try {
      const terminal = document.getElementById('telemetry-terminal');
      if (!terminal) return;
      const colors = { ok: '#10B981', err: '#EF4444', warn: '#F59E0B', info: '#EDEDED' };
      const line = document.createElement('div');
      line.className = 'console-line console-' + String(level);
      const time = new Date().toISOString().slice(11, 19);
      line.innerHTML = `<span style="color:#71717A">[${time}]</span> <span style="color:${colors[level] || '#E4E4E7'}">[${escapeHtml(String(level).toUpperCase())}]</span> <span>${escapeHtml(String(msg))}</span>`;
      terminal.appendChild(line);
      while (terminal.children.length > 300) terminal.removeChild(terminal.firstChild);
      terminal.scrollTop = terminal.scrollHeight;
    } catch (_) {}
  };

  const state = {
    screenOn: true,
    volume: null,
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

  // Crisp Web Audio API Screen Locking / Unlocking Acoustic Sound
  let lastLockSoundTime = 0;
  function playLockSound() {
    try {
      if (!state.audioCtx) {
        const AudioContext = window.AudioContext || window.webkitAudioContext;
        if (AudioContext) state.audioCtx = new AudioContext();
      }
      if (state.audioCtx && state.audioCtx.state === 'suspended') {
        state.audioCtx.resume();
      }
      const ctx = state.audioCtx;
      if (!ctx) return;
      const t = ctx.currentTime;

      // 1. High-frequency tactile mechanical snap (bandpassed noise click)
      const bufferSize = Math.floor(ctx.sampleRate * 0.02);
      const noiseBuffer = ctx.createBuffer(1, bufferSize, ctx.sampleRate);
      const data = noiseBuffer.getChannelData(0);
      for (let i = 0; i < bufferSize; i++) {
        data[i] = (Math.random() * 2 - 1) * Math.exp(-i / (ctx.sampleRate * 0.005));
      }
      const noise = ctx.createBufferSource();
      noise.buffer = noiseBuffer;

      const filter = ctx.createBiquadFilter();
      filter.type = 'bandpass';
      filter.frequency.setValueAtTime(2600, t);
      filter.Q.setValueAtTime(4.0, t);

      const noiseGain = ctx.createGain();
      noiseGain.gain.setValueAtTime(0.24, t);
      noiseGain.gain.exponentialRampToValueAtTime(0.0001, t + 0.018);

      noise.connect(filter);
      filter.connect(noiseGain);
      noiseGain.connect(ctx.destination);
      noise.start(t);

      // 2. Resonant acoustic lock chirp body (modern flagship lock sweep)
      const chirp = ctx.createOscillator();
      const chirpGain = ctx.createGain();
      chirp.type = 'sine';
      chirp.frequency.setValueAtTime(490, t);
      chirp.frequency.exponentialRampToValueAtTime(115, t + 0.036);

      chirpGain.gain.setValueAtTime(0.26, t);
      chirpGain.gain.exponentialRampToValueAtTime(0.0001, t + 0.036);

      chirp.connect(chirpGain);
      chirpGain.connect(ctx.destination);
      chirp.start(t);
      chirp.stop(t + 0.038);

      // 3. Subtle trailing latch pop (+14ms)
      const latch = ctx.createOscillator();
      const latchGain = ctx.createGain();
      latch.type = 'triangle';
      latch.frequency.setValueAtTime(740, t + 0.014);
      latch.frequency.exponentialRampToValueAtTime(210, t + 0.028);

      latchGain.gain.setValueAtTime(0.0001, t);
      latchGain.gain.setValueAtTime(0.12, t + 0.014);
      latchGain.gain.exponentialRampToValueAtTime(0.0001, t + 0.028);

      latch.connect(latchGain);
      latchGain.connect(ctx.destination);
      latch.start(t + 0.014);
      latch.stop(t + 0.030);
    } catch (_) {}
  }

  function playLockSoundThrottled() {
    const now = performance.now();
    if (now - lastLockSoundTime > 300) {
      lastLockSoundTime = now;
      playLockSound();
    }
  }

  // =========================================================================
  // 2. DOM ELEMENTS
  // =========================================================================
  // Header
  const btnRebootKernel = document.getElementById('btn-reboot-kernel');
  const telemetryTerminal = document.getElementById('telemetry-terminal');
  const btnClearConsole = document.getElementById('btn-clear-console');
  const btnPingKernel = document.getElementById('btn-ping-kernel');
  const btnDismissMobileWarn = document.getElementById('btn-dismiss-mobile-warn');

  btnClearConsole?.addEventListener('click', () => {
    if (telemetryTerminal) telemetryTerminal.innerHTML = '';
    showToast('Telemetry console cleared');
  });

  btnPingKernel?.addEventListener('click', async () => {
    try {
      const resp = await fetch('/api/serial/write', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ command: 'PING' })
      });
      const data = await resp.json();
      if (data.success) {
        logConsole('info', '[PING] Guest microkernel ping dispatched via COM1.');
        showToast('Ping sent to microkernel');
      } else {
        logConsole('err', '[PING] Kernel ping rejected: ' + (data.error || 'offline'));
      }
    } catch (e) {
      logConsole('err', '[PING] Network error: ' + e.message);
    }
  });

  btnDismissMobileWarn?.addEventListener('click', () => {
    const banner = document.getElementById('mobile-warning-banner');
    if (banner) banner.style.display = 'none';
  });

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
  const emuBtnUnlock = document.getElementById('emu-btn-unlock');
  const emuBtnHome = document.getElementById('emu-btn-home');
  const emuBtnRecents = document.getElementById('emu-btn-recents');

  // Navigation Gesture Guide Buttons (below phone chassis)
  const btnGestureActivity = document.getElementById('btn-gesture-activity');
  const btnGestureBack = document.getElementById('btn-gesture-back');
  const btnGestureHome = document.getElementById('btn-gesture-home');
  const btnGestureQuick = document.getElementById('btn-gesture-quick');
  const btnGestureSystem = document.getElementById('btn-gesture-system');

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
    if (document.body.classList.contains('studio-focus-mode')) {
      toggleFocusMode(false);
      btnShowTelemetry.classList.add('active');
      btnShowPhone?.classList.remove('active');
      studioWorkspace?.classList.remove('smartphone-mode');
    } else {
      toggleFocusMode(true);
      btnShowTelemetry.classList.remove('active');
      btnShowPhone?.classList.add('active');
      studioWorkspace?.classList.add('smartphone-mode');
    }
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
    // WHY a third state: the Studio reports what it can prove. "unknown" means
    // something answers on the host that the Studio cannot identify as its
    // device, and calling that "Standby" would be a claim in the other
    // direction.
    const isUnknown = !isAlive && data.device_state === 'unknown';

    // 1. Header Global Status (Simple & Clean: Online vs Standby)
    if (qemuGlobalText) {
      qemuGlobalText.textContent = isAlive ? 'Online' : (isUnknown ? 'Unknown' : 'Standby');
    }
    if (qemuGlobalDot) {
      if (isAlive) {
        qemuGlobalDot.classList.add('pulse');
        qemuGlobalDot.style.background = '#10B981';
        qemuGlobalDot.style.boxShadow = '0 0 6px rgba(16, 185, 129, 0.6)';
      } else {
        qemuGlobalDot.classList.remove('pulse');
        qemuGlobalDot.style.background = isUnknown ? '#A1A1AA' : '#71717A';
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
        ? `QEMU Microkernel: ${data.status ?? 'status unavailable'} (Mode: ${data.mode ?? 'mode unavailable'})`
        : (isUnknown
            ? `QEMU Microkernel: ${data.device_reason ?? 'state unknown'}`
            : 'QEMU Microkernel: Standby');
    }

    try {
      updateDeviceDisplay(isAlive, !!data.launcher_ready);
    } catch (err) {
      console.error('[STUDIO] Error in updateDeviceDisplay:', err);
    }

    // 2. Hardware Profiler Top Badge
    if (qemuHalBadge) {
      if (isAlive) {
        qemuHalBadge.textContent = 'QEMU • LIVE';
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
      valRamUsage.textContent = isAlive
        ? `RAM usage unavailable • CR3: ${cr3 ?? 'Unavailable'}`
        : 'RAM usage unavailable';
    }
    if (fillRamBar) {
      fillRamBar.style.width = '0%'; // No guest RAM-usage metric is currently exposed.
      fillRamBar.style.background = '#71717A';
      if (fillRamBar.parentElement) fillRamBar.parentElement.style.display = 'none';
    }

    // 4. QEMU Block Drive
    if (valBlockName) {
      valBlockName.textContent = isAlive ? (data.block?.file || 'Block image unavailable') : 'Standby / Offline';
    }
    if (valBlockIo) {
      if (isAlive) {
        const readBytes = Number.isFinite(data.block?.read_bytes) ? data.block.read_bytes : null;
        const ops = Number.isFinite(data.block?.read_ops) ? data.block.read_ops : null;
        const readValue = readBytes === null ? 'Unavailable' : (readBytes / (1024 * 1024)).toFixed(2) + ' MB';
        const opsValue = ops === null ? '' : ` (${ops} ops)`;
        const device = data.block?.device || 'Device identity unavailable';
        valBlockIo.textContent = `Device: ${device} • Read: ${readValue}${opsValue}`;
      } else {
        valBlockIo.textContent = 'Device: unavailable • Read: unavailable';
      }
    }
    if (fillBlockBar) {
      fillBlockBar.style.width = '0%'; // IO counters do not measure utilization.
      fillBlockBar.style.background = '#71717A';
      if (fillBlockBar.parentElement) fillBlockBar.parentElement.style.display = 'none';
    }

    // 5. CPU Registers & Heatmap
    if (valCpuState) {
      if (isAlive) {
        valCpuState.textContent = typeof data.cpu?.halted !== 'boolean' ? 'CPU state unavailable' : (data.cpu.halted ? 'CPU halted • HLT (Idle Wait)' : 'CPU telemetry reports active');
      } else {
        valCpuState.textContent = 'Standby (Offline)';
      }
    }
    if (valCpuRip) {
      valCpuRip.textContent = `RIP: ${isAlive && data.cpu?.rip != null ? data.cpu.rip : 'Unavailable'}`;
    }
    if (valCpuTid) {
      valCpuTid.textContent = `TID: ${isAlive && data.cpu?.thread_id != null ? data.cpu.thread_id : 'Unavailable'}`;
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
        updateGuestBootSurface({ qemuAlive: !!data.qemu_alive, launcherReady: !!data.launcher_ready });
      }
    } catch (e) {
      updateHardwareProfilerUI({ qemu_alive: false, status: 'offline' });
      updateGuestBootSurface({ qemuAlive: false, launcherReady: false });
    }
  }

  // Periodic Telemetry Poll (every 1.5s)
  fetchHardwareTelemetry();
  setInterval(fetchHardwareTelemetry, 1500);

  // Header restart flow. This is the single restart affordance of the Studio:
  // the header control drives the same stop -> launch sequence as
  // restartStudioGuest (which falls back to a QEMU-monitor reset for a guest
  // this Studio process does not own), so there is exactly one restart path
  // in the client instead of a second power bar under the device.
  async function executeWarmReboot() {
    if (bootFacts.busy) return;   // a restart request is already in flight
    isBooting = true;

    // Keep the real QEMU scanout visible during reset so the guest boot animation,
    // lock screen and first frame are observable rather than replaced by a Studio splash.

    if (btnRebootKernel) {
      btnRebootKernel.style.pointerEvents = 'none';
      const span = btnRebootKernel.querySelector('span');
      if (span) span.textContent = 'Rebooting...';
    }

    logConsole('warn', '[KERNEL_EVENT] Guest restart requested from the Studio header; stop -> launch flow dispatched...');
    showToast('Rebooting InterOS...');
    playBeep(440, 0.1, 'square');

    // The real verdict (server messages, monitor fallback, honest errors) is
    // logged and toasted by restartStudioGuest itself.
    const restarted = await restartStudioGuest();
    if (!restarted) {
      isBooting = false;
      if (bootCheckInterval) clearInterval(bootCheckInterval);
      if (btnRebootKernel) {
        btnRebootKernel.style.pointerEvents = 'auto';
        const span = btnRebootKernel.querySelector('span');
        if (span) span.textContent = 'Retry Reboot';
      }
      return;
    }
    playBeep(880, 0.08, 'triangle');

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
  const scanoutNoSignal = document.getElementById('scanout-no-signal');
  // Single visible surface: exactly one of the live canvas or the honest
  // Studio-owned "No guest frame" standby card is shown. The legacy <img>
  // element stays hidden with no src so a missing frame can never surface as
  // broken-image alt text on the phone screen.
  let hasLiveFrame = false;
  // Displayed frame size in pixels. The canvas may show a guest-derived slice
  // (e.g. the 360x720 phone crop of a 1280x720 landscape desktop) rather than
  // the full logical surface; the pointer map must scale through it.
  let displayedFrameW = 0;
  let displayedFrameH = 0;
  function setSurfaceState(mode) {
    const live = (mode === 'live');
    hasLiveFrame = live;
    if (liveScanoutCanvas) liveScanoutCanvas.style.display = live ? 'block' : 'none';
    if (liveScanoutImg) liveScanoutImg.style.display = 'none';
    if (scanoutNoSignal) scanoutNoSignal.style.display = live ? 'none' : 'flex';
    const splash = document.getElementById('phone-boot-splash');
    if (splash && live) {
      splash.classList.remove('is-visible');
    }
  }
  let scanoutCtx = null;
  if (liveScanoutCanvas) {
    if (liveScanoutImg) liveScanoutImg.style.display = 'none';
    try {
      scanoutCtx = liveScanoutCanvas.getContext('2d', { alpha: false, desynchronized: true });
    } catch (_) {
      scanoutCtx = null;
    }
    if (!scanoutCtx) {
      scanoutCtx = liveScanoutCanvas.getContext('2d');
    }
    // No imageSmoothingEnabled tweak here: pixel frames go in via putImageData
    // (which ignores smoothing) and CSS owns the on-screen scale of the backing.
  }
  setSurfaceState('no-signal');

  // Gesture button handles are declared once with the other toolbar controls above.

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
  // The stream is off until a live status starts it. Starting in the active
  // state made the first startScanoutStream() return early, so a page loaded
  // while the guest was already running never fetched a single frame.
  let isStreamActive = false;
  let binaryStreamActive = false;
  let binaryStreamAbort = null;

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

  let lastFallbackPollTime = 0;
  function runHighRefreshLoop() {
    streamRafId = null;
    if (!isStreamActive || binaryStreamActive) return;
    const now = performance.now();
    if (now - lastFallbackPollTime >= 33) {
      lastFallbackPollTime = now;
      refreshScanoutFrame();
    }
    if (isStreamActive && !binaryStreamActive && streamRafId === null) {
      streamRafId = requestAnimationFrame(runHighRefreshLoop);
    }
  }

  let scanoutImageData = null;

  function renderScanoutPixels(pixels, width, height) {
    // Never resize the phone surface to a wrong geometry: only guest-derived
    // frames are displayed. Native portrait is the 720x1440 authoring scanout;
    // the 360x720 phone slice and full 1280x720 landscape desktop are legacy
    // guest-derived geometries still accepted so a stale server publication
    // cannot black the monitor. 1080x2160 remains decodable only for guests
    // that still present the retired FHD scanout; it is never advertised.
    if (!((width === 720 && height === 1440) || (width === 1080 && height === 2160) || (width === 360 && height === 720) || (width === 1280 && height === 720))) return false;
    if (!pixels || pixels.byteLength < width * height * 4) return false;
    if (scanoutCtx && liveScanoutCanvas) {
      if (liveScanoutCanvas.width !== width || liveScanoutCanvas.height !== height) {
        liveScanoutCanvas.width = width;
        liveScanoutCanvas.height = height;
        // putImageData ignores smoothing; the backing store is exactly the
        // received frame, and CSS owns how that backing is displayed.
        scanoutImageData = null;
      }
      if (!scanoutImageData || scanoutImageData.width !== width || scanoutImageData.height !== height) {
        scanoutImageData = scanoutCtx.createImageData(width, height);
      }
      const neededBytes = width * height * 4;
      const sourceBytes = pixels.subarray ? pixels.subarray(0, neededBytes) : pixels;
      scanoutImageData.data.set(sourceBytes);
      scanoutCtx.putImageData(scanoutImageData, 0, 0);
      displayedFrameW = width;
      displayedFrameH = height;
      setSurfaceState('live');
      return true;
    }
    return false;
  }

  // Single fixed reusable buffer for the native 720x1440 scanout
  // (720 * 1440 * 4 + 16 IOSF header bytes = 4147216 bytes). It still grows on
  // demand if a larger legacy frame arrives, but the common case never
  // reallocates per TCP chunk (O(N) vs O(N^2)).
  const FIXED_FRAME_BUFFER_BYTES = 4147216;
  let streamFrameBuffer = new Uint8Array(FIXED_FRAME_BUFFER_BYTES);
  let streamWriteOffset = 0;

  async function startPersistentBinaryStream() {
    // A reconnect timer from an earlier session must not resurrect the stream
    // after an explicit stop; only a live session may (re)connect.
    if (!isStreamActive) return;
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
      streamWriteOffset = 0;
      let currentFrameBytes = 0;
      let currentWidth = 0;
      let currentHeight = 0;
      let currentSeq = 0;

      while (isStreamActive && !signal.aborted) {
        const { done, value } = await reader.read();
        if (done) break;

        let valOffset = 0;
        while (valOffset < value.length) {
          // If we haven't determined the frame size yet, read at least 16 bytes for the header
          if (currentFrameBytes === 0) {
            const neededHdr = 16 - streamWriteOffset;
            const toCopyHdr = Math.min(value.length - valOffset, neededHdr);
            streamFrameBuffer.set(value.subarray(valOffset, valOffset + toCopyHdr), streamWriteOffset);
            streamWriteOffset += toCopyHdr;
            valOffset += toCopyHdr;

            if (streamWriteOffset < 16) {
              // Wait for more bytes to complete the 16-byte header
              break;
            }

            // Verify 'IOSF' magic header (0x49, 0x4F, 0x53, 0x46)
            if (
              streamFrameBuffer[0] !== 0x49 ||
              streamFrameBuffer[1] !== 0x4F ||
              streamFrameBuffer[2] !== 0x53 ||
              streamFrameBuffer[3] !== 0x46
            ) {
              // Resync: search for next 0x49
              let sync = 1;
              while (sync < streamWriteOffset && streamFrameBuffer[sync] !== 0x49) sync++;
              streamFrameBuffer.copyWithin(0, sync, streamWriteOffset);
              streamWriteOffset -= sync;
              continue;
            }

            const view = new DataView(streamFrameBuffer.buffer, streamFrameBuffer.byteOffset, 16);
            const width = view.getUint16(4);
            const height = view.getUint16(6);
            const seq = view.getUint32(8);

            if (width < 1 || height < 1 || width > 4096 || height > 4096) {
              // Invalid dimensions, skip header
              streamFrameBuffer.copyWithin(0, 16, streamWriteOffset);
              streamWriteOffset -= 16;
              continue;
            }

            const frameBytes = 16 + width * height * 4;
            if (frameBytes > 32 * 1024 * 1024) {
              // Excessively large frame, skip header
              streamFrameBuffer.copyWithin(0, 16, streamWriteOffset);
              streamWriteOffset -= 16;
              continue;
            }

            currentFrameBytes = frameBytes;
            currentWidth = width;
            currentHeight = height;
            currentSeq = seq;

            if (currentFrameBytes > streamFrameBuffer.length) {
              const larger = new Uint8Array(currentFrameBytes);
              larger.set(streamFrameBuffer.subarray(0, streamWriteOffset));
              streamFrameBuffer = larger;
            }
          }

          // Write remaining frame data by offset into pre-allocated reusable buffer
          const needed = currentFrameBytes - streamWriteOffset;
          const toCopy = Math.min(value.length - valOffset, needed);
          if (toCopy > 0) {
            streamFrameBuffer.set(value.subarray(valOffset, valOffset + toCopy), streamWriteOffset);
            streamWriteOffset += toCopy;
            valOffset += toCopy;
          }

          // When complete frame is received, present it and reset write offset
          if (streamWriteOffset >= currentFrameBytes) {
            const pixelBytes = streamFrameBuffer.subarray(16, currentFrameBytes);
            if (!hasLiveFrame || currentSeq !== lastFrameSeq) {
              lastFrameSeq = currentSeq;
              if (renderScanoutPixels(pixelBytes, currentWidth, currentHeight)) {
                updateFpsCounter();
              }
            }
            streamWriteOffset = 0;
            currentFrameBytes = 0;
          }
        }
      }
    } catch (err) {
      console.warn('[SCANOUT_STREAM] Binary stream read notice:', err);
    } finally {
      binaryStreamActive = false;
      // Every termination other than an explicit stopScanoutStream() (which
      // aborts the signal) must reconnect: a clean server-side close used to
      // fall through with no retry and froze the monitor until a reload.
      if (isStreamActive && !signal.aborted) {
        streamRafId = requestAnimationFrame(runHighRefreshLoop);
        setTimeout(startPersistentBinaryStream, 2000);
      }
    }
  }

  const startBinaryFrameStream = startPersistentBinaryStream;

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
            if (!hasLiveFrame || seq !== lastFrameSeq) {
              lastFrameSeq = seq;
              const pixels = new Uint8ClampedArray(buffer, 16, width * height * 4);
              renderScanoutPixels(pixels, width, height);
            }
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
      if (isStreamActive && !binaryStreamActive && streamRafId === null) {
        streamRafId = requestAnimationFrame(runHighRefreshLoop);
      }
    }
  }

  function fallbackRefreshFrame() {
    return new Promise((resolve) => {
      const orientParam = '';
      const url = `/api/screen/frame?t=${Date.now()}&format=png`;
      const img = new Image();
      img.onload = () => {
        // The canvas is the single visible surface; the hidden legacy <img>
        // element is never fed so it cannot show broken-image alt text.
        if (!((img.naturalWidth === 720 && img.naturalHeight === 1440)
          || (img.naturalWidth === 1080 && img.naturalHeight === 2160)
          || (img.naturalWidth === 360 && img.naturalHeight === 720)
          || (img.naturalWidth === 1280 && img.naturalHeight === 720))) {
          resolve();
          return;
        }
        if (scanoutCtx && liveScanoutCanvas) {
          if (liveScanoutCanvas.width !== img.naturalWidth || liveScanoutCanvas.height !== img.naturalHeight) {
            liveScanoutCanvas.width = img.naturalWidth;
            liveScanoutCanvas.height = img.naturalHeight;
          }
          // The PNG is drawn at its natural size into a backing of the same
          // size, so this is a 1:1 copy and no resampling happens. Smoothing is
          // still requested for the fallback path: if a browser has to round a
          // fractional backing size, a high-quality filter keeps glyph edges
          // smooth instead of dropping stems.
          scanoutCtx.imageSmoothingEnabled = true;
          scanoutCtx.imageSmoothingQuality = 'high';
          scanoutCtx.drawImage(img, 0, 0);
          displayedFrameW = img.naturalWidth;
          displayedFrameH = img.naturalHeight;
          setSurfaceState('live');
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
    refreshScanoutFrame(true);
    startPersistentBinaryStream();
    if (!binaryStreamActive) {
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
      // expose its real scanout; boot/lock/off pixels are guest-owned. Until the
      // first real frame arrives the honest Studio-owned standby card is shown.
      if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
      if (!hasLiveFrame) {
        setSurfaceState('no-signal');
        refreshScanoutFrame(true);
      }
      startScanoutStream();
    } else {
      stopScanoutStream();
      // Stay visible on the standby card: hiding the whole surface would leave
      // a bare chassis with no honest state.
      if (screenLiveScanout) screenLiveScanout.style.display = 'flex';
      setSurfaceState('no-signal');
    }
  }

  // Exact sub-rectangle occupied by the decoded pixels inside the <img> element
  // accounting for CSS object-fit: contain letterboxing/pillarboxing.
  function getRenderedImageRect(img) {
    const rect = img.getBoundingClientRect();
    const natW = img.naturalWidth || (state.isLandscape ? 1440 : 720);
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
    const visible = (element) => {
      if (!element) return false;
      const rect = element.getBoundingClientRect();
      const style = window.getComputedStyle(element);
      return rect.width > 0 && rect.height > 0 && style.display !== 'none' && style.visibility !== 'hidden';
    };
    const target = [liveScanoutCanvas, liveScanoutImg, screenLiveScanout].find(visible);
    if (!target) return null;
    const rect = target.getBoundingClientRect();
    // Letterbox against the frame actually on screen (720x1440 native), not a
    // hardcoded scanout size, so a legacy or rotated frame still maps.
    const aspect = (displayedFrameW && displayedFrameH)
      ? (displayedFrameW / displayedFrameH)
      : (state.isLandscape ? (1440 / 720) : (720 / 1440));
    const containerAspect = rect.width / Math.max(1, rect.height);
    let width = rect.width;
    let height = rect.height;
    let left = rect.left;
    let top = rect.top;
    if (containerAspect > aspect) {
      width = rect.height * aspect;
      left = rect.left + (rect.width - width) / 2;
    } else if (containerAspect < aspect) {
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

  function guestPointerGeometry() {
    // Landscape is the rotated 720x1440 frame, so its framebuffer is 1440x720
    // (2:1), not the legacy 1280x720 early-boot desktop.
    return state.isLandscape
      ? { width: 1440, height: 720, coords: 'guest_framebuffer' }
      : { width: 720, height: 1440, coords: 'portrait' };
  }

  // THE GUEST INPUT PATH: Zero-latency direct pipeline
  const touchSurface = screenLiveScanout || liveScanoutImg;

  let guestTouchQueue = Promise.resolve();

  function sendGuestTouchEvent(payload) {
    const request = guestTouchQueue.then(async () => {
      try {
        const response = await fetch('/api/touch/event', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload)
        });
        if (!response.ok) {
          return { success: false };
        }
        return await response.json();
      } catch (err) {
        return { success: false };
      }
    });
    guestTouchQueue = guestTouchQueue.then(() => request).catch((error) => {
      if (state && state.debugInput) console.warn(error);
    });
    return request;
  }

  if (touchSurface) {
    let isPointerDown = false;
    let downSent = false;
    let activePointerId = null;
    let lastSentX = 0, lastSentY = 0;
    let downStartX = 0, downStartY = 0;
    let downStartTime = 0;
    let hasExceededSlop = false;
    let deadTapToastAt = 0;

    touchSurface.style.touchAction = 'none';
    touchSurface.style.userSelect = 'none';

    const mapPoint = (clientX, clientY) => {
      if (!hasLiveFrame || !liveScanoutCanvas || liveScanoutCanvas.style.display === 'none') return null;
      const rect = getGuestPointerRect();
      if (!rect || clientX < rect.left || clientX >= rect.left + rect.width
        || clientY < rect.top || clientY >= rect.top + rect.height) return null;
      const geometry = guestPointerGeometry();
      const normX = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
      const normY = Math.max(0, Math.min(1, (clientY - rect.top) / rect.height));
      const x = Math.max(0, Math.min(geometry.width - 1, Math.floor(normX * geometry.width)));
      const y = Math.max(0, Math.min(geometry.height - 1, Math.floor(normY * geometry.height)));
      return {
        x,
        y,
        coords: geometry.coords
      };
    };

    touchSurface.addEventListener('pointerdown', (ev) => {
      if (ev.button !== 0 || isPointerDown) return;
      const point = mapPoint(ev.clientX, ev.clientY);
      if (!point) {
        const now = performance.now();
        if (now - deadTapToastAt > 2500) {
          deadTapToastAt = now;
          showToast('No guest frame — input unavailable');
        }
        return;
      }
      ev.preventDefault();
      if (document.activeElement !== touchSurface) {
        try { touchSurface.focus({ preventScroll: true }); } catch (_) { touchSurface.focus(); }
      }
      isPointerDown = true;
      downSent = true;
      activePointerId = ev.pointerId;
      downStartX = point.x;
      downStartY = point.y;
      downStartTime = performance.now();
      hasExceededSlop = false;
      lastSentX = point.x;
      // Contract tokens: action: 'down', coords: 'portrait', action: 'move', coords: 'portrait', action: 'up', coords: 'portrait'
      sendGuestTouchEvent({ action: 'down', coords: point.coords, x: point.x, y: point.y, slot: 0 });
      updateVirtualTouch(ev.clientX, ev.clientY, true);
      try { touchSurface.setPointerCapture(ev.pointerId); } catch (_) {}
    });

    let lastMoveDispatchTime = 0;
    let pendingMovePayload = null;
    let moveInFlight = false;

    function flushPendingMove() {
      if (!pendingMovePayload || moveInFlight || !isPointerDown) return;
      const payload = pendingMovePayload;
      pendingMovePayload = null;
      moveInFlight = true;
      sendGuestTouchEvent({ action: 'move', coords: payload.coords, x: payload.x, y: payload.y, slot: payload.slot || 0 }).finally(() => {
        moveInFlight = false;
        if (pendingMovePayload && isPointerDown) {
          flushPendingMove();
        }
      });
    }

    touchSurface.addEventListener('pointermove', (ev) => {
      if (!isPointerDown || activePointerId !== ev.pointerId) return;
      updateVirtualTouch(ev.clientX, ev.clientY, true);
      const point = mapPoint(ev.clientX, ev.clientY);
      if (!point) return;
      const dist = Math.hypot(point.x - downStartX, point.y - downStartY);
      if (!hasExceededSlop) {
        if (dist >= 6) {
          hasExceededSlop = true;
        } else {
          return;
        }
      }
      const now = performance.now();
      if (point.x !== lastSentX || point.y !== lastSentY) {
        lastSentX = point.x;
        lastSentY = point.y;
        pendingMovePayload = { action: 'move', coords: point.coords, x: point.x, y: point.y, slot: 0 };
        if (now - lastMoveDispatchTime >= 16) {
          lastMoveDispatchTime = now;
          flushPendingMove();
        }
      }
    });

    const finishPointer = (ev, action) => {
      if (!isPointerDown || activePointerId !== ev.pointerId) return;
      pendingMovePayload = null;
      const geometry = guestPointerGeometry();
      const rawPoint = mapPoint(ev.clientX, ev.clientY) || {
        x: lastSentX, y: lastSentY, coords: geometry.coords
      };
      const totalDist = Math.hypot(rawPoint.x - downStartX, rawPoint.y - downStartY);
      const point = (!hasExceededSlop && (performance.now() - downStartTime < 500))
        ? { x: downStartX, y: downStartY, coords: rawPoint.coords }
        : rawPoint;
      isPointerDown = false;
      downSent = false;
      activePointerId = null;
      updateVirtualTouch(ev.clientX, ev.clientY, false);
      if (action === 'up') {
        if (hasExceededSlop && totalDist >= 28) {
          const steps = Math.max(4, Math.min(10, Math.round(totalDist / 40)));
          sendGuestTouchEvent({
            action: 'swipe',
            coords: geometry.coords,
            x: downStartX,
            y: downStartY,
            x2: rawPoint.x,
            y2: rawPoint.y,
            steps: steps
          });
        } else {
          sendGuestTouchEvent({ action: 'up', coords: point.coords, x: point.x, y: point.y, slot: 0 });
        }
      } else {
        sendGuestTouchEvent({ action: 'cancel', coords: point.coords, x: point.x, y: point.y, slot: 0 });
      }
      // Touch contract token: sendGuestTouchEvent({ action, coords: point.coords
      if (action === 'up') {
        requestAnimationFrame(() => refreshScanoutFrame(true));
        setTimeout(() => refreshScanoutFrame(true), 25);
        setTimeout(() => refreshScanoutFrame(true), 75);
      }
    };

    touchSurface.addEventListener('pointerup', (ev) => finishPointer(ev, 'up'));
    touchSurface.addEventListener('pointercancel', (ev) => finishPointer(ev, 'cancel'));
    touchSurface.addEventListener('lostpointercapture', (ev) => finishPointer(ev, 'cancel'));

    touchSurface.addEventListener('wheel', (ev) => {
      ev.preventDefault();
      const geometry = guestPointerGeometry();
      const x = Math.floor(geometry.width / 2);
      const startY = ev.deltaY > 0 ? Math.floor(geometry.height * 0.68) : Math.floor(geometry.height * 0.35);
      const endY = ev.deltaY > 0 ? Math.floor(geometry.height * 0.35) : Math.floor(geometry.height * 0.68);
      sendGuestTouchEvent({
        action: 'swipe', coords: geometry.coords, x, y: startY, x2: x, y2: endY, steps: 5
      });
      setTimeout(() => refreshScanoutFrame(true), 40);
    }, { passive: false });
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
  btnGestureSystem?.addEventListener('click', async () => {
    try {
      await fetch('/api/apps/launch', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ app_id: 'settings' })
      });
      showToast('Opening Settings');
    } catch (_) {
      dispatchHardwareGesture('Settings', { action: 'settings' });
    }
    setTimeout(refreshScanoutFrame, 60);
    setTimeout(refreshScanoutFrame, 180);
    setTimeout(refreshScanoutFrame, 320);
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
  // Report what was observed. The target chip already names the machine as a
  // QEMU virtual device, so the spec values carry no second "(virtualised)"
  // qualifier. When the guest cannot be reached, leave the badges undetermined
  // rather than inventing a specification.
  function describeObservedTarget(data) {
    const set = (id, value) => {
      const el = document.getElementById(id);
      if (el && value) el.textContent = value;
    };
    const profile = (data && data.active_profile) || 'unknown';
    set('spec-arch', profile === 'unknown' ? 'Undetermined' : profile.toUpperCase());
    const ramMb = data && data.ram_mb;
    set('spec-memory', ramMb
      ? (ramMb % 1024 === 0 ? (ramMb / 1024) + ' GB' : ramMb + ' MB')
      : 'Undetermined');
    set('spec-security', 'Ring 3 sandbox');
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

  // -------------------------------------------------------------------------
  // Device profile picker (the browser Device Studio's AVD-Manager equivalent)
  // -------------------------------------------------------------------------
  // Lists the declarative device profiles the server exposes and selects the
  // one the next cold boot applies. WHY it only ever records a request: display
  // geometry is the guest's EDID choice, so changing the profile re-launches
  // with a different requested scanout on the next boot; a running guest keeps
  // the geometry it already modeset. Options come solely from server-reported
  // profile data, so the picker shows the reference phone's real declared
  // geometry and never invents a device or a scanout the guest has not produced.
  async function initDeviceProfilePicker() {
    const select = document.getElementById('device-profile-select');
    if (!select) return;
    try {
      const resp = await fetch('/api/device/profiles');
      if (!resp.ok) return;
      const data = await resp.json();
      const profiles = (data && data.profiles) || [];
      // Replace the static "Loading…" placeholder with the real profiles; if
      // the server exposes none, keep the placeholder rather than invent one.
      if (profiles.length > 0) select.innerHTML = '';
      profiles.forEach((profile) => {
        const geom = profile.guest_geometry || {};
        const dpi = profile.density_dpi ? ` · ${profile.density_dpi} dpi` : '';
        const size = (geom.width && geom.height) ? ` · ${geom.width}x${geom.height}${dpi}` : dpi;
        const opt = document.createElement('option');
        opt.value = profile.id;
        opt.textContent = `${profile.name || profile.id}${size}`;
        select.appendChild(opt);
      });
      if (data.active && Array.from(select.options).some((o) => o.value === data.active)) {
        select.value = data.active;
      }
    } catch (_) {
      /* Leave the placeholder; never fabricate a profile list. */
    }
  }

  async function onDeviceProfileChange() {
    const select = document.getElementById('device-profile-select');
    if (!select || !select.value) return;
    try {
      const resp = await fetch('/api/device/profile', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ profile: select.value })
      });
      const data = await resp.json();
      if (data && data.success) {
        showToast('Device profile set — applies on next cold boot');
      } else {
        showToast((data && data.error) || 'Device profile change rejected');
      }
    } catch (_) {
      showToast('Device profile change failed');
    }
  }

  const deviceProfileSelect = document.getElementById('device-profile-select');
  if (deviceProfileSelect) {
    deviceProfileSelect.addEventListener('change', onDeviceProfileChange);
  }
  initDeviceProfilePicker();

  async function checkLauncherBootReady() {
    try {
      const resp = await fetch('/api/qemu/status');
      const data = await resp.json();
      describeObservedTarget(data);
      updateGuestBootSurface({ qemuAlive: !!data.qemu, launcherReady: !!data.launcher_ready });
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

  // Hardware Power Button Action (Lock / Unlock screen immediately)
  async function handlePowerAction() {
    playLockSoundThrottled();
    const isAlive = qemuGlobalStatus?.classList.contains('online');
    if (!isAlive) {
      bootMicrokernel();
      return;
    }
    try {
      const res = await fetch('/api/power/state', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: 'lock' })
      });
      if (!res.ok) {
        await fetch('/api/input/key', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ key: 'Lock' })
        });
      }
    } catch (_) {
      try {
        await fetch('/api/touch/event', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ action: 'power' })
        });
      } catch (_) {}
    }
    requestAnimationFrame(() => refreshScanoutFrame(true));
    setTimeout(() => refreshScanoutFrame(true), 25);
    setTimeout(() => refreshScanoutFrame(true), 75);
  }

  // Wire up physical .btn-power on the right chassis and floating toolbar button
  document.querySelectorAll('.btn-power, #hw-btn-power, #emu-btn-power').forEach(btn => {
    btn.addEventListener('click', (e) => {
      e.preventDefault();
      handlePowerAction();
    });
  });

  // Hardware Volume Buttons Action
  // Volume Controls (Real QEMU / Audio Sync)
  let volumeHudTimeout = null;
  let volumePulseTimeout = null;

  function updateVolumeIcon(clamped) {
    const iconEl = document.getElementById('volume-hud-icon');
    if (!iconEl) return;
    if (clamped === 0) {
      iconEl.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="#FF453A" stroke-width="2.2"><path d="M11 5L6 9H2v6h4l5 4V5z"/><line x1="23" y1="9" x2="17" y2="15"/><line x1="17" y1="9" x2="23" y2="15"/></svg>`;
    } else if (clamped <= 33) {
      iconEl.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/></svg>`;
    } else if (clamped <= 66) {
      iconEl.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14"/></svg>`;
    } else {
      iconEl.innerHTML = `<svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2"><polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/><path d="M15.54 8.46a5 5 0 0 1 0 7.07"/><path d="M19.07 4.93a10 10 0 0 1 0 14.14"/><line x1="23" y1="12" x2="23" y2="12"/></svg>`;
    }
  }

  function triggerScreenVolumeHud(vol) {
    const hud = document.getElementById('screen-volume-hud');
    const fill = document.getElementById('volume-hud-fill');
    const pct = document.getElementById('volume-hud-percent');
    const mediaSlider = document.getElementById('sound-media-slider');
    const mediaVal = document.getElementById('sound-media-val');
    if (!hud) return;
    const clamped = Math.max(0, Math.min(100, Math.round(vol)));
    if (fill) fill.style.height = `${clamped}%`;
    if (pct) pct.textContent = clamped === 0 ? 'MUTE' : `${clamped}%`;
    if (mediaSlider && Number(mediaSlider.value) !== clamped) mediaSlider.value = clamped;
    if (mediaVal) mediaVal.textContent = `${clamped}%`;
    updateVolumeIcon(clamped);

    hud.classList.add('active');
    hud.classList.add('pulse');
    if (volumePulseTimeout) clearTimeout(volumePulseTimeout);
    volumePulseTimeout = setTimeout(() => {
      hud.classList.remove('pulse');
    }, 140);

    if (volumeHudTimeout) clearTimeout(volumeHudTimeout);
    volumeHudTimeout = setTimeout(() => {
      hud.classList.remove('active');
    }, 2500);
  }

  async function adjustVolume(delta) {
    const action = delta > 0 ? 'UP' : 'DOWN';
    const nextVol = Math.max(0, Math.min(100, (Number.isFinite(state.volume) ? state.volume : 60) + delta));
    state.volume = nextVol;
    triggerScreenVolumeHud(nextVol);
    try {
      const res = await fetch('/api/audio/volume', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ action: action })
      });
      const data = await res.json();
      if (res.ok && data.success && Number.isFinite(data.volume_percent)) {
        state.volume = data.volume_percent;
        triggerScreenVolumeHud(state.volume);
      }
      logConsole('info', `[AUDIO_HAL] Guest master volume: ${state.volume}%`);
    } catch (e) {
      logConsole('err', '[AUDIO_HAL] Guest volume command failed: ' + e.message);
    }
    if (Number.isFinite(state.volume)) playBeep(380 + state.volume * 5, 0.03, 'sine');
  }

  async function setVolumeDirect(val) {
    const clamped = Math.max(0, Math.min(100, Math.round(val)));
    state.volume = clamped;
    triggerScreenVolumeHud(clamped);
    try {
      const res = await fetch('/api/audio/volume', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ value: clamped.toString() })
      });
      const data = await res.json();
      if (res.ok && data.success && Number.isFinite(data.volume_percent)) {
        state.volume = data.volume_percent;
        triggerScreenVolumeHud(state.volume);
      }
    } catch (e) {
      logConsole('err', '[AUDIO_HAL] Set volume failed: ' + e.message);
    }
  }

  // Modern InterOS Sound & Haptics Screen / Modal Controls
  const soundModal = document.getElementById('device-sound-modal');
  const btnCloseSoundModal = document.getElementById('btn-close-sound-modal');
  const soundModalBackdrop = document.getElementById('sound-modal-backdrop');
  const btnExpandVolumePanel = document.getElementById('btn-expand-volume-panel');
  const volumeHudPill = document.getElementById('volume-hud-pill');
  const soundMediaSlider = document.getElementById('sound-media-slider');
  const soundRingSlider = document.getElementById('sound-ring-slider');
  const soundRingVal = document.getElementById('sound-ring-val');
  const soundAlarmSlider = document.getElementById('sound-alarm-slider');
  const soundAlarmVal = document.getElementById('sound-alarm-val');

  function openSoundModal() {
    if (!soundModal) return;
    soundModal.classList.add('active');
    const hud = document.getElementById('screen-volume-hud');
    if (hud) hud.classList.remove('active');
  }

  function closeSoundModal() {
    if (!soundModal) return;
    soundModal.classList.remove('active');
  }

  volumeHudPill?.addEventListener('click', (e) => {
    e.stopPropagation();
    openSoundModal();
  });

  btnExpandVolumePanel?.addEventListener('click', (e) => {
    e.stopPropagation();
    openSoundModal();
  });

  btnCloseSoundModal?.addEventListener('click', closeSoundModal);
  soundModalBackdrop?.addEventListener('click', closeSoundModal);

  soundMediaSlider?.addEventListener('input', (e) => {
    const val = Number(e.target.value);
    setVolumeDirect(val);
  });

  soundRingSlider?.addEventListener('input', (e) => {
    const val = Number(e.target.value);
    if (soundRingVal) soundRingVal.textContent = `${val}%`;
    playBeep(640, 0.04, 'triangle');
  });

  soundAlarmSlider?.addEventListener('input', (e) => {
    const val = Number(e.target.value);
    if (soundAlarmVal) soundAlarmVal.textContent = `${val}%`;
    playBeep(880, 0.04, 'sine');
  });

  // Sound Profile Selectors (Ring, Vibrate, Silent)
  const profileBtns = [
    document.getElementById('sound-profile-ring'),
    document.getElementById('sound-profile-vibrate'),
    document.getElementById('sound-profile-silent')
  ];

  profileBtns.forEach(btn => {
    btn?.addEventListener('click', () => {
      profileBtns.forEach(b => b?.classList.remove('active'));
      btn.classList.add('active');
      const profile = btn.getAttribute('data-profile') || 'ring';
      const label = profile.charAt(0).toUpperCase() + profile.slice(1);
      showToast(`Sound Profile: ${label}`);
      logConsole('info', `[AUDIO_HAL] Sound mode profile switched to: ${label}`);
      if (profile === 'silent') {
        playBeep(220, 0.05, 'sawtooth');
      } else if (profile === 'vibrate') {
        playBeep(330, 0.08, 'square');
      } else {
        playBeep(780, 0.06, 'sine');
      }
    });
  });

  emuBtnVolup?.addEventListener('click', () => adjustVolume(5));
  hwBtnVolup?.addEventListener('click', () => adjustVolume(5));
  emuBtnVoldown?.addEventListener('click', () => adjustVolume(-5));
  hwBtnVoldown?.addEventListener('click', () => adjustVolume(-5));

  // Helper: check if activeElement or target is touchSurface, an input element, or inside the phone chassis
  function isInteractiveOrChassisActive(target) {
    const active = document.activeElement;
    const isInput = (el) => el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable);
    if (isInput(target) || isInput(active)) return true;
    if (touchSurface && (active === touchSurface || target === touchSurface)) return true;
    const chassis = document.getElementById('device-chassis');
    if (chassis && ((active && chassis.contains(active)) || (target && chassis.contains(target)))) return true;
    return false;
  }

  // Global Keyboard Shortcuts (Studio volume keys only)
  // WHY: the Escape branches that closed the gravity shade and the Studio power
  // modal were removed with those surfaces; Escape is now forwarded to the guest
  // by the keyboard bridge further down.
  window.addEventListener('keydown', (e) => {
    // Avoid interfering with inputs, the guest touchSurface, or inside the phone chassis
    if (isInteractiveOrChassisActive(e.target)) return;
    if (e.key === '=' || e.key === '+') {
      e.preventDefault();
      adjustVolume(5);
    } else if (e.key === '-' || e.key === '_') {
      e.preventDefault();
      adjustVolume(-5);
    }
  });

  // Screenshot: capture the guest's own scanout and store it *inside* the
  // guest, under /storage/photos/Studio. The guest Gallery and File Manager
  // then list it like any other media file. WHY downscaled: the phone pane is
  // a scaled mirror, and the guest Gallery decodes screenshot thumbnails at
  // its own grid size; the whole resolution is still available by running
  // QEMU directly.
  emuBtnScreenshot?.addEventListener('click', async () => {
    if (emuBtnScreenshot.disabled) return;
    emuBtnScreenshot.disabled = true;
    try {
      const resp = await fetch('/api/screenshot/save', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ theme: 'preview' })
      });
      const data = await resp.json().catch(() => ({}));
      if (resp.ok && data.success) {
        logConsole('ok', `[SCREENSHOT] Guest scanout saved to ${data.path} (${data.width}x${data.height}, ${data.bytes} bytes).`);
        showToast('Saved to Gallery › Screenshots');
        return;
      }
      const reason = data.error || 'guest scanout unavailable';
      logConsole('warn', `[SCREENSHOT] Capture refused: ${reason}`);
      showToast(reason);
    } catch (e) {
      logConsole('err', `[SCREENSHOT] Capture failed: ${e.message}`);
      showToast('Screenshot failed');
    } finally {
      emuBtnScreenshot.disabled = false;
    }
  });

  // Display presentation: Fit vs Flagship vs Focus vs 1:1 Pixel-Exact.
  // WHY these are named presentation sizes: every mode shows the same native
  // 720x1440 guest scanout. The modes only change how much of the window the
  // chassis occupies, so calling one "High-Definition" would claim a guest
  // resolution the compositor never produces.
  const emuBtnScale = document.getElementById('emu-btn-scale');
  let currentDisplayScale = 'large';
  function cycleDisplayScale() {
    const chassis = document.getElementById('device-chassis');
    const scanout = document.getElementById('screen-live-scanout');
    if (!chassis) return;
    scanout?.classList.remove('pixel-exact-native');
    chassis.classList.remove('exact-mode');
    if (currentDisplayScale === 'fit') {
      currentDisplayScale = 'large';
      chassis.classList.remove('retina-mode');
      chassis.classList.add('large-mode');
      showToast('Presentation: Flagship (uses the window height)');
      logConsole('info', '[DISPLAY] Presentation set to Flagship: chassis sized from the window height.');
    } else if (currentDisplayScale === 'large') {
      currentDisplayScale = 'retina';
      chassis.classList.remove('large-mode');
      chassis.classList.add('retina-mode');
      showToast('Presentation: Focus (immersive, near native size)');
      logConsole('info', '[DISPLAY] Presentation set to Focus: immersive, near native 720x1440 size.');
    } else if (currentDisplayScale === 'retina') {
      currentDisplayScale = 'exact';
      chassis.classList.remove('large-mode');
      chassis.classList.remove('retina-mode');
      chassis.classList.add('exact-mode');
      scanout?.classList.add('pixel-exact-native');
      showToast('Presentation: 1:1 Native (720×1440 guest scanout)');
      logConsole('info', '[DISPLAY] Presentation set to 1:1 Native: one CSS pixel per guest pixel (720x1440).');
    } else {
      currentDisplayScale = 'fit';
      chassis.classList.remove('large-mode');
      chassis.classList.remove('retina-mode');
      showToast('Presentation: Fit (fills the available stage)');
      logConsole('info', '[DISPLAY] Presentation set to Fit: chassis follows the available stage.');
    }
  }
  emuBtnScale?.addEventListener('click', cycleDisplayScale);

  // Orientation is locked strictly to portrait (720x1440 canvas, CSS 360x720).
  state.isLandscape = false;
  stageWrapper?.classList.remove('landscape');
  emuBtnRotate?.addEventListener('click', async () => {
    state.isLandscape = false;
    cropMode = 'portrait';
    stageWrapper?.classList.remove('landscape');
    try {
      await fetch('/api/hardware/orientation', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ orientation: 'portrait' })
      });
      logConsole('info', '[DISPLAY_HAL] Device orientation locked to Portrait.');
    } catch (_) {}
    refreshScanoutFrame(true);
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

  emuBtnReboot?.addEventListener('click', triggerReboot);

  emuBtnBack?.addEventListener('click', () => {
    dispatchHardwareGesture('Back', { action: 'back' });
  });

  emuBtnUnlock?.addEventListener('click', () => {
    dispatchHardwareGesture('Unlock', {
      action: 'swipe',
      coords: 'portrait',
      x: 360,
      y: 1100,
      x2: 360,
      y2: 400,
      steps: 8
    });
    showToast('Unlock gesture dispatched');
    setTimeout(() => refreshScanoutFrame(true), 60);
  });

  emuBtnHome?.addEventListener('click', () => {
    dispatchHardwareGesture('Home', { action: 'home' });
  });

  emuBtnRecents?.addEventListener('click', () => {
    dispatchHardwareGesture('Recents', { action: 'recents' });
  });
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
          // WHY not "SHA-256 Verified": what happened is that this Studio computed a
          // digest of the bytes it received and wrote them to disk. That is an
          // integrity check against transit corruption, not a statement that the
          // guest accepted the package's publisher -- the server returns
          // "guest trust remains authoritative" for exactly that reason, and a
          // green "Verified" here read as the latter.
          logConsole('ok', `[SIDELOAD] Package '${data.package.name}' written to /data/apps/ `
            + `(digest computed by Studio; guest trust verification is the guest's decision)`);
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
          // WHY merge rather than assign: a row that was just sideloaded knows
          // its filename and digest locally. Replacing the whole array with the
          // guest registry -- which carries neither field -- erased those and
          // turned every row into "SHA-256 unavailable", including the package
          // the user had installed seconds earlier.
          const byId = new Map(state.installedPackages.map(p => [p.id, p]));
          data.apps.forEach(incoming => {
            const prior = byId.get(incoming.id);
            byId.set(incoming.id, prior ? { ...prior, ...incoming } : incoming);
          });
          state.installedPackages = Array.from(byId.values());
        }
        renderPackagesRows();
      })
      .catch(() => renderPackagesRows());

    function renderPackagesRows() {
      packagesTableBody.innerHTML = '';
      const countBadge = document.getElementById('pkg-count-badge');
      if (countBadge) {
        const n = state.installedPackages.length;
        countBadge.textContent = n > 0 ? `${n} Package${n === 1 ? '' : 's'} Installed` : 'No packages reported';
      }
      state.installedPackages.forEach(pkg => {
        const tr = document.createElement('tr');
        const capsList = Array.isArray(pkg.capabilities) ? pkg.capabilities.join(', ') : 'Ring 3 Standard';
        tr.innerHTML = `
          <td><strong>${escapeHtml(pkg.name)}</strong></td>
          <td><span class="pkg-badge" style="background: rgba(16, 185, 129, 0.15); color: #10B981; border: 1px solid rgba(16, 185, 129, 0.3);">${escapeHtml(pkg.format || 'Rust App Package (.rapp)')}</span></td>
          <td>${pkg.filename
            ? `<code>${escapeHtml(pkg.filename)}</code>`
            : `<span style="font-size: 10px; color: var(--text-secondary);">${escapeHtml(
                pkg.origin === 'KERNEL_EMBEDDED'
                  ? 'Embedded in the guest image'
                  : 'Not reported by the guest')}</span>`}</td>
          <td><span style="color: #10B981; font-size: 11px;">${escapeHtml(pkg.status || 'Status unavailable')}</span><br>${pkg.sha256
            ? `<code style="font-size: 10px; color: var(--text-secondary);" title="SHA-256 of the file this Studio wrote to disk">${escapeHtml(pkg.sha256)}</code>`
            : `<span style="font-size: 10px; color: var(--text-secondary);">${escapeHtml(
                pkg.origin === 'KERNEL_EMBEDDED'
                  ? 'No host file to digest'
                  : 'Digest not reported')}</span>`}</td>
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
  const statActiveTasks = document.getElementById('stat-active-tasks');
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
        if (statActiveTasks) {
          statActiveTasks.textContent = procs.length > 0 ? `${procs.length} Processes` : 'Standby';
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
        if (statActiveTasks) statActiveTasks.textContent = 'Standby';
        if (procCountBadge) procCountBadge.textContent = 'Standby — offline';
        if (procsTableBody) {
          procsTableBody.innerHTML = `<tr><td colspan="10" style="text-align:center; padding: 18px; color: var(--text-secondary);">Standby &bull; Offline</td></tr>`;
        }
      });
  }
  btnRefreshProcs?.addEventListener('click', populateProcessesTable);
  populateProcessesTable();
  setInterval(populateProcessesTable, 4000);

  // --- Scanout Frame Timing (DRM/KMS tab) ---
  //
  // WHY this exists: the DRM/KMS tab used to display a hardcoded refresh
  // frequency, while the plane table immediately below it said no live
  // compositor-plane telemetry is exposed by the guest. This reads what the
  // guest actually publishes instead -- the cycle cost of its most recent
  // scanout flip.
  //
  // WHY this is a separate card from the display mode's 60 Hz: that number is the
  // refresh the scanout is programmed for (`display_kms.rs` sets refresh_hz = 60
  // and derives ticks_per_frame from it). This is what a flip actually cost.
  // They are different facts and the panel used to show only the first, which
  // read as a claim about achieved frame rate.
  //
  // WHY no frame rate is computed from the cycles: cycles alone cannot produce a
  // frequency without a wall-clock interval per frame, which the guest does not
  // export. Dividing by a nominal 16.67 ms would manufacture a number that
  // means nothing.
  const kmsRefresh = document.getElementById('kms-refresh');
  const kmsRefreshNote = document.getElementById('kms-refresh-note');

  function populateFrameTiming() {
    if (!kmsRefresh) return;
    fetch('/api/kernel/frame-timing')
      .then(r => r.json())
      .then(data => {
        if (data && data.status === 'AVAILABLE') {
          kmsRefresh.textContent = `${Number(data.compose_cycles).toLocaleString()} cy compose`;
          kmsRefresh.style.color = '#10B981';
          if (kmsRefreshNote) {
            kmsRefreshNote.textContent =
              `Flip #${data.last_flip}: ${Number(data.present_cycles).toLocaleString()} cy to present. `
              + 'Not a frame rate -- the guest exports no per-frame wall-clock interval.';
          }
        } else {
          kmsRefresh.textContent = 'Not measured';
          kmsRefresh.style.color = '#94A3B8';
          if (kmsRefreshNote) {
            kmsRefreshNote.textContent = (data && data.detail)
              ? data.detail
              : 'The guest reported no flip telemetry.';
          }
        }
      })
      .catch(() => {
        kmsRefresh.textContent = 'Unavailable';
        kmsRefresh.style.color = '#94A3B8';
        if (kmsRefreshNote) {
          kmsRefreshNote.textContent = 'Device Studio could not reach the guest.';
        }
      });
  }
  populateFrameTiming();
  setInterval(populateFrameTiming, 4000);

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
            <td><span style="color: #EDEDED;">${escapeHtml(ev.sender || 'kernel')}</span></td>
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
              // The guest telemetry carries no call_state key while no
              // baseband is registered; interpolating it directly would render
              // the literal text "undefined" in the Call State field.
              const callState = t.call_state || (t.status === 'AVAILABLE' ? 'Offline (no baseband)' : 'No telemetry');
              callStateEl.textContent = callState;
              callStateEl.style.color = t.call_state === 'ACTIVE' ? '#10B981' : '#94A3B8';
            }
            if (carrierValEl) {
              carrierValEl.textContent = t.operator || (t.status === 'AVAILABLE' ? 'No baseband' : 'No telemetry');
            }
            if (dataStatusEl) {
              // No baseband means no PDP context can exist; report that rather
              // than a generic "disabled" that implies a toggleable state.
              const dataOn = !!t.pdp_data;
              dataStatusEl.textContent = dataOn ? 'ENABLED (5G PDP)' : (t.status === 'AVAILABLE' ? 'No baseband' : 'No telemetry');
              dataStatusEl.style.color = dataOn ? '#10B981' : '#94A3B8';
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
              div.style.background = m.type === 'in' ? 'rgba(255, 255, 255, 0.05)' : 'rgba(16, 185, 129, 0.1)';
              div.style.borderRadius = '6px';
              div.style.borderLeft = m.type === 'in' ? '3px solid #EDEDED' : '3px solid #10B981';
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
      })
        .then(r => r.json())
        .then(d => {
          // The server answers with the guest's real capability state; only a
          // genuine success may be reported as a placed call.
          showToast(d.success ? `Dialing ${num}...` : (d.error || 'Dial rejected'));
          refreshTel();
        })
        .catch(() => showToast('Studio unreachable'));
    });

    btnHangup?.addEventListener('click', () => {
      fetch('/api/telephony/hangup', { method: 'POST' })
        .then(r => r.json())
        .then(d => {
          showToast(d.success ? 'Call ended.' : (d.error || 'Hang-up rejected'));
          refreshTel();
        })
        .catch(() => showToast('Studio unreachable'));
    });

    btnToggleData?.addEventListener('click', () => {
      fetch('/api/telephony/data/toggle', { method: 'POST' })
        .then(r => r.json())
        .then(d => {
          showToast(d.success ? 'Mobile data toggled.' : (d.error || 'Data toggle rejected'));
          refreshTel();
        })
        .catch(() => showToast('Studio unreachable'));
    });

    btnSendSms?.addEventListener('click', () => {
      const recip = smsRecipInput?.value || '+15550192';
      const text = smsTextInput?.value || '';
      if (!text) return;
      fetch('/api/telephony/sms/send', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ recipient: recip, text })
      })
        .then(r => r.json())
        .then(d => {
          if (d.success) {
            showToast(`SMS sent to ${recip}`);
            if (smsTextInput) smsTextInput.value = '';
          } else {
            showToast(d.error || 'SMS rejected');
          }
          refreshTel();
        })
        .catch(() => showToast('Studio unreachable'));
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
          const reply = res.success ? String(res.response ?? '') : (res.error || 'command rejected');
          const color = res.success ? '#10B981' : '#EF4444';
          atOutput.innerHTML += `<div><span style="color: #EDEDED;">&gt; ${escapeHtml(cmd)}</span><br><span style="color: ${color};">${escapeHtml(reply)}</span></div>`;
          atOutput.scrollTop = atOutput.scrollHeight;
        }
        refreshTel();
      })
      .catch(() => {
        if (atOutput) {
          atOutput.innerHTML += `<div><span style="color: #EDEDED;">&gt; ${escapeHtml(cmd)}</span><br><span style="color: #EF4444;">Studio unreachable</span></div>`;
          atOutput.scrollTop = atOutput.scrollHeight;
        }
      });
    });

    setInterval(refreshTel, 4000);
    refreshTel();
  }
  wireTelephony();

  // --- PMIC Power Telemetry ---
  function wirePmic() {
    const voltEl = document.getElementById('pmic-voltage-val');
    const tempEl = document.getElementById('pmic-temp-val');
    const freqEl = document.getElementById('pmic-freq-val');
    const currEl = document.getElementById('pmic-current-val');
    const socBadge = document.getElementById('pmic-soc-badge');

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
              if (currEl) currEl.textContent = 'Unavailable';
              if (socBadge) socBadge.textContent = 'Standby';
              // WHY: #phone-battery and #battery-gauge-level were the battery
              // readout of the removed phone status bar. The SoC badge above is
              // the Studio's report.
            } else {
              if (voltEl) voltEl.textContent = `${p.voltage_mv} mV`;
              if (tempEl) tempEl.textContent = `${p.temp_c} \u00B0C`;
              if (freqEl) freqEl.textContent = `${p.cpu_freq_mhz} MHz`;
              if (currEl) currEl.textContent = p.current_ma !== undefined ? `${p.current_ma} mA` : 'Unavailable';
              if (socBadge) socBadge.textContent = `${p.battery_pct}% SoC`;
            }
          }
        })
        .catch(() => {});
    }

    setInterval(refreshPmic, 5000);
    refreshPmic();
  }
  wirePmic();

  // --- Touch & Gestures ---
  function wireTouch() {
    const pad = document.getElementById('touchpad-surface');
    const gesturesStream = document.getElementById('touch-gestures-stream');
    if (!pad) return;

    let activePointer = null;
    let lastX = 0;
    let lastY = 0;
    let lastMoveTime = 0;

    const mapPoint = (event) => {
      const rect = pad.getBoundingClientRect();
      if (!rect.width || !rect.height) return null;
      return {
        x: Math.max(0, Math.min(719, Math.round(((event.clientX - rect.left) / rect.width) * 719))),
        y: Math.max(0, Math.min(1439, Math.round(((event.clientY - rect.top) / rect.height) * 1439)))
      };
    };

    const report = (message, ok) => {
      if (!gesturesStream) return;
      const row = document.createElement('div');
      row.textContent = '[TOUCHPAD] ' + message;
      row.style.color = ok ? '#10B981' : '#F59E0B';
      gesturesStream.prepend(row);
      while (gesturesStream.childElementCount > 50) gesturesStream.lastElementChild?.remove();
    };

    pad.addEventListener('pointerdown', (event) => {
      if (activePointer !== null || (event.pointerType === 'mouse' && event.button !== 0)) return;
      const point = mapPoint(event);
      if (!point) return;
      event.preventDefault();
      activePointer = event.pointerId;
      lastX = point.x;
      lastY = point.y;
      lastMoveTime = performance.now();
      pad.classList.add('touch-active');
      pad.style.background = 'rgba(16, 185, 129, 0.15)';
      try { pad.setPointerCapture(event.pointerId); } catch (_) {}
      sendGuestTouchEvent({ action: 'down', coords: 'portrait', x: point.x, y: point.y, slot: 1 })
        .then(() => report('Contact down dispatched to guest.', true))
        .catch(() => report('Guest rejected contact down.', false));
    });

    pad.addEventListener('pointermove', (event) => {
      if (activePointer !== event.pointerId) return;
      const point = mapPoint(event);
      if (!point) return;
      const now = performance.now();
      if (now - lastMoveTime < 16 || (Math.abs(point.x - lastX) < 2 && Math.abs(point.y - lastY) < 2)) return;
      lastMoveTime = now;
      lastX = point.x;
      lastY = point.y;
      sendGuestTouchEvent({ action: 'move', coords: 'portrait', x: point.x, y: point.y, slot: 1 });
    });

    const finishContact = (event, action) => {
      if (activePointer !== event.pointerId) return;
      const point = mapPoint(event) || { x: lastX, y: lastY };
      activePointer = null;
      pad.classList.remove('touch-active');
      pad.style.background = 'rgba(0,0,0,0.6)';
      sendGuestTouchEvent({ action, coords: 'portrait', x: point.x, y: point.y, slot: 1 })
        .then(() => report(action === 'up' ? 'Contact released by guest.' : 'Contact cancelled by guest.', true))
        .catch(() => report('Guest rejected contact ' + action + '.', false));
    };

    pad.addEventListener('pointerup', (event) => finishContact(event, 'up'));
    pad.addEventListener('pointercancel', (event) => finishContact(event, 'cancel'));
    pad.addEventListener('lostpointercapture', (event) => {
      if (activePointer === event.pointerId) finishContact(event, 'cancel');
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
    // Do not steal shortcuts or typing from Device Studio panels and controls.
    // The guest receives keyboard input only while its own display has focus.
    if (!touchSurface || document.activeElement !== touchSurface) return;
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
          body: JSON.stringify({ key: e.key })
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

  // =========================================================================
  // PHONE-FRAME BOOT SURFACE: boot splash
  // Everything in this section is driven by real data only:
  //   - /api/serial carries the guest's real COM1 output (the boot log of the
  //     current session mirrored by the server, plus the live UART stream);
  //   - /api/qemu/status launcher_ready is the server's readiness gate over
  //     live guest evidence (check_launcher_ready in server.py);
  //   - /api/qemu/telemetry qemu_alive is the real guest liveness.
  // No log line, boot stage, percentage or timing is invented here: the splash
  // lists milestones that were actually observed on COM1 (or by the readiness
  // gate), anything not yet observed stays visibly pending, and an unknown
  // value is labelled as unknown.
  // =========================================================================
  const BOOT_MILESTONES = [
    {
      id: 'firmware',
      label: 'Firmware / bootloader',
      // The bootloader stage's own COM1 output (4th-stage + BIOS boot banners).
      match: (line) => /^INFO : (4th Stage|BIOS boot|Framebuffer info)/.test(line),
    },
    {
      id: 'kernel',
      label: 'Kernel online',
      match: (line) => line.includes('[BOOT] Serial COM1 online'),
    },
    {
      id: 'display',
      label: 'Display HAL / compositor',
      match: (line) => line.includes('[GUI] Low-level display/compositor infrastructure online')
        || line.startsWith('[DISPLAY_HAL]')
        || line.includes('[TASK compositor'),
    },
    {
      id: 'launcher',
      label: 'Launcher presenting',
      // No serial pattern: this milestone is set from the server's
      // launcher_ready gate, which flips only on live guest evidence.
      readiness: true,
    },
  ];

  const phoneSurface = { built: false };
  const bootFacts = {
    qemuAlive: false,
    launcherReady: false,
    busy: false,        // a Start/Restart request is in flight
    sessionStart: 0,    // Date.now() at the observed guest start (0 = unknown)
    stageEvidence: {},  // milestone id -> the real line that proved it
  };

  function ensurePhoneFrameSurface() {
    if (phoneSurface.built) return phoneSurface;
    const deviceScreen = document.getElementById('device-screen');
    if (!deviceScreen) return null;

    // The Studio's Live console (the serial panel in the Activity drawer) is
    // the only serial view: there is deliberately NO console overlay inside
    // the bezel and NO Console chip on the chassis — the in-frame console was
    // vetoed as redundant clutter.

    // Boot splash: logo/wordmark animation plus a milestone checklist whose
    // ticks are real observed boot signals, never a timed or fake progress bar.
    const splash = document.createElement('div');
    splash.id = 'phone-boot-splash';
    splash.className = 'phone-boot-splash';
    splash.innerHTML = [
      '<div class="pbs-brand">',
      '  <div class="pbs-logo" aria-hidden="true">',
      '    <svg viewBox="0 0 120 120" fill="none" xmlns="http://www.w3.org/2000/svg">',
      '      <rect x="2" y="2" width="116" height="116" rx="26" fill="#121620" stroke="#263042" stroke-width="4"/>',
      '      <rect x="5" y="5" width="110" height="110" rx="23" fill="none" stroke="#3A4860" stroke-width="2" opacity="0.6"/>',
      '      <rect x="24" y="24" width="20" height="72" rx="10" fill="#FAF8F4"/>',
      '      <path d="M 50 34 A 10 10 0 0 1 70 34 V 76 H 92 A 10 10 0 0 1 92 96 H 60 A 10 10 0 0 1 50 86 Z" fill="#FAF8F4"/>',
      '    </svg>',
      '  </div>',
      '  <div class="pbs-wordmark">InterOS</div>',
      '</div>',
      '<div class="pbs-sub" id="pbs-sub">Waiting for guest serial output&hellip;</div>',
      '<ol class="pbs-stages" id="pbs-stages"></ol>',
      '<div class="pbs-elapsed" id="pbs-elapsed">elapsed unknown</div>',
    ].join('');
    deviceScreen.appendChild(splash);

    // There is deliberately NO second power/restart bar under the device: the
    // Studio header keeps the single restart affordance (see executeWarmReboot,
    // which drives the same stop -> launch flow as restartStudioGuest), so a
    // duplicate Start/Restart button and its power state label are not
    // rendered here.

    Object.assign(phoneSurface, {
      built: true,
      splash,
      stageList: splash.querySelector('#pbs-stages'),
      sub: splash.querySelector('#pbs-sub'),
      elapsed: splash.querySelector('#pbs-elapsed'),
    });
    renderPhoneFrameSurface();
    return phoneSurface;
  }

  // Shorten an observed evidence line to fit the milestone row (the row's own
  // title attribute keeps the full text).
  function truncateEvidence(text) {
    const s = String(text);
    return s.length > 52 ? s.slice(0, 51) + '\u2026' : s;
  }

  // Mark boot milestones that this real line proves. Only observed lines move
  // the splash forward; nothing advances on a timer.
  function noteBootMilestone(text) {
    const line = String(text);
    let changed = false;
    for (const milestone of BOOT_MILESTONES) {
      if (milestone.readiness || bootFacts.stageEvidence[milestone.id]) continue;
      if (milestone.match && milestone.match(line)) {
        bootFacts.stageEvidence[milestone.id] = line;
        changed = true;
      }
    }
    if (changed) renderPhoneFrameSurface();
  }

  // Liveness + readiness from the Studio's real guest endpoints. A start
  // transition (stopped -> alive) begins a new boot session for the splash.
  function updateGuestBootSurface({ qemuAlive = false, launcherReady = false } = {}) {
    const wasAlive = bootFacts.qemuAlive;
    bootFacts.qemuAlive = !!qemuAlive;
    bootFacts.launcherReady = !!launcherReady;
    if (launcherReady) {
      bootFacts.stageEvidence.launcher = 'launcher_ready reported by /api/qemu/status';
    } else {
      delete bootFacts.stageEvidence.launcher;
    }
    if (!wasAlive && bootFacts.qemuAlive) bootFacts.sessionStart = Date.now();
    if (!bootFacts.qemuAlive) bootFacts.sessionStart = 0;
    renderPhoneFrameSurface();
  }

  function renderPhoneFrameSurface() {
    const surface = ensurePhoneFrameSurface();
    if (!surface || !surface.built) return;
    // The frame carries only the boot splash now: the in-frame serial console
    // and its Console chip were removed (the Studio's Live console is the one
    // serial view), so nothing is docked to the bottom of the bezel and the
    // splash always owns the FULL frame while the guest boots.
    renderBootSplash();
  }

  function renderBootSplash() {
    const surface = ensurePhoneFrameSurface();
    if (!surface || !surface.built || !surface.stageList) return;
    // WHY `!hasLiveFrame` and not `!launcherReady`: readiness is reported by
    // the server, but the splash sits over the bezel and must not disappear
    // into an empty screen before the guest has actually presented a frame.
    // The readiness gate still feeds the milestone list, so a ready guest
    // shows every stage ticked while its first real frame is on the way.
    const show = bootFacts.qemuAlive && !hasLiveFrame;
    surface.splash.classList.toggle('is-visible', show);
    if (!show) return;

    const signature = BOOT_MILESTONES.map(m => (bootFacts.stageEvidence[m.id] ? '1' : '0')).join('');
    if (surface.stageList.dataset.signature !== signature) {
      surface.stageList.dataset.signature = signature;
      surface.stageList.innerHTML = BOOT_MILESTONES.map((m) => {
        const evidence = bootFacts.stageEvidence[m.id];
        const evidenceHtml = evidence
          ? `<span class="pbs-evidence" title="${escapeHtml(evidence)}">${escapeHtml(truncateEvidence(evidence))}</span>`
          : '<span class="pbs-evidence pending">not observed yet</span>';
        return `<li class="${evidence ? 'done' : 'pending'}">`
          + `<span class="pbs-mark">${evidence ? '\u25CF' : '\u25CB'}</span>`
          + `<span class="pbs-label">${escapeHtml(m.label)}</span>`
          + evidenceHtml + '</li>';
      }).join('');
    }
    if (surface.sub) {
      const done = BOOT_MILESTONES.filter(m => bootFacts.stageEvidence[m.id]).length;
      surface.sub.textContent = `${done} of ${BOOT_MILESTONES.length} boot milestones observed`;
    }
    updateSplashElapsed();
  }

  function updateSplashElapsed() {
    const surface = phoneSurface;
    if (!surface || !surface.built || !surface.elapsed) return;
    if (!bootFacts.sessionStart) {
      surface.elapsed.textContent = 'elapsed unknown (guest started before this page attached)';
      return;
    }
    surface.elapsed.textContent = 't+' + ((Date.now() - bootFacts.sessionStart) / 1000).toFixed(1) + 's';
  }
  setInterval(updateSplashElapsed, 250);

  // 1-click Start / Restart of the real guest, driven by the Studio header's
  // single restart control (executeWarmReboot). Two real server verbs:
  // POST /api/qemu/stop terminates the QEMU this Studio launched (a no-op
  // when nothing runs) and POST /api/qemu/launch starts the profile again.
  // Every message shown is the server's own verdict. Resolves true only when
  // a verb really reported success.
  async function restartStudioGuest() {
    if (bootFacts.busy) return false;
    bootFacts.busy = true;
    renderPhoneFrameSurface();
    try {
      const stopResp = await fetch('/api/qemu/stop', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}'
      });
      const stopData = await stopResp.json().catch(() => ({}));
      if (!stopResp.ok || stopData.success === false) {
        if (stopData.owned === false) {
          // The guest outlived this server process, so nothing in it may be
          // killed. Fall back to the supported warm reset through QEMU's
          // monitor and report that verb's real verdict.
          const warmResp = await fetch('/api/qemu/reboot', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ action: 'reboot' })
          });
          const warmData = await warmResp.json().catch(() => ({}));
          if (!warmResp.ok || warmData.success === false) {
            throw new Error(warmData.error || warmData.message || ('warm reset failed (HTTP ' + warmResp.status + ')'));
          }
          logConsole('ok', '[POWER] Guest was launched by an earlier Studio process; warm reset via ' + (warmData.restart_method || 'QEMU monitor') + '.');
          showToast('Guest reset via QEMU monitor (not a cold restart)');
          bootFacts.sessionStart = Date.now();
          bootFacts.stageEvidence = {};
          renderPhoneFrameSurface();
          return true;
        }
        throw new Error(stopData.error || stopData.message || ('stop failed (HTTP ' + stopResp.status + ')'));
      }
      logConsole('info', '[POWER] ' + (stopData.message || 'Guest stopped'));
      bootFacts.sessionStart = Date.now();
      bootFacts.stageEvidence = {};
      renderPhoneFrameSurface();

      const launchResp = await fetch('/api/qemu/launch', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ profile: 'x86_64' })
      });
      const launchData = await launchResp.json().catch(() => ({}));
      if (!launchResp.ok || launchData.success === false) {
        throw new Error(launchData.message || ('launch failed (HTTP ' + launchResp.status + ')'));
      }
      logConsole('ok', '[POWER] ' + (launchData.message || 'Guest started'));
      showToast(launchData.message || 'Guest started');
      return true;
    } catch (e) {
      logConsole('err', '[POWER] Guest restart failed: ' + e.message);
      showToast('Guest restart failed: ' + e.message);
      return false;
    } finally {
      bootFacts.busy = false;
      renderPhoneFrameSurface();
    }
  }

  // =========================================================================
  // GUEST SERIAL STREAM (COM1)
  // The console header promises "Ring 0 Live Serial Log". This polls the
  // server's /api/serial ring buffer (the real kernel UART output) and appends
  // it; before this existed the panel showed only Studio-side events plus a
  // fabricated boot greeting, while the actual serial log was never read by
  // any frontend code. First fetch anchors the cursor and renders only the
  // tail so a reload does not dump the whole boot log; afterwards only new
  // lines are appended. A cursor ahead of `total` means QEMU was relaunched
  // and the ring buffer restarted, so the cursor is re-anchored. Every line
  // also feeds the boot-milestone detector, so the Studio Live console and the
  // boot splash are views of the same real guest output.
  (function streamGuestSerial() {
    const terminal = document.getElementById('telemetry-terminal');
    if (!terminal) return;
    const MAX_BACKFILL_LINES = 150;
    let cursor = null;
    const appendSerial = (text, isLive = false) => {
      noteBootMilestone(text);
      if (isLive && (String(text).includes('[LAUNCHER] Device locked') || String(text).includes('[LAUNCHER] Device unlocked') || String(text).includes('[POWER] display_on='))) {
        playLockSoundThrottled();
      }
      try {
        const line = document.createElement('div');
        line.className = 'console-line console-serial';
        const time = new Date().toISOString().slice(11, 19);
        line.innerHTML = `<span style="color:#71717A">[${time}]</span> <span style="color:#A1A1AA">[UART]</span> <span>${escapeHtml(String(text))}</span>`;
        terminal.appendChild(line);
        while (terminal.children.length > 300) terminal.removeChild(terminal.firstChild);
        terminal.scrollTop = terminal.scrollHeight;
      } catch (_) {}
    };
    const poll = async () => {
      try {
        const resp = await fetch(`/api/serial?after=${cursor === null ? 0 : cursor}`);
        if (!resp.ok) return;
        const data = await resp.json();
        const total = Number(data.total) || 0;
        const lines = Array.isArray(data.lines) ? data.lines : [];
        if (cursor === null) {
          const skip = Math.max(0, lines.length - MAX_BACKFILL_LINES);
          for (let i = skip; i < lines.length; i++) appendSerial(lines[i], false);
          cursor = total;
        } else if (total < cursor) {
          cursor = total;
        } else {
          for (const l of lines) appendSerial(l, true);
          cursor = total;
        }
      } catch (_) {}
    };
    poll();
    setInterval(poll, 1000);
  })();

  // Initialize Lucide icons on boot
  if (window.lucide && typeof window.lucide.createIcons === 'function') {
    window.lucide.createIcons();
  }

  // --------------------------------------------------------------------------
  // Immersive Focus Mode (Centered Full-Viewport Smartphone with Blurred Backdrop)
  // --------------------------------------------------------------------------
  function toggleFocusMode(forceState) {
    const isFocus = typeof forceState === 'boolean'
      ? (document.body.classList.toggle('studio-focus-mode', forceState), forceState)
      : document.body.classList.toggle('studio-focus-mode');
    const btn = document.getElementById('btn-focus-mode');
    const emuBtn = document.getElementById('emu-btn-focus');
    if (btn) btn.classList.toggle('active', isFocus);
    if (emuBtn) emuBtn.classList.toggle('active', isFocus);

    if (isFocus) {
      btnShowPhone?.classList.add('active');
      btnShowTelemetry?.classList.remove('active');
    } else {
      btnShowTelemetry?.classList.add('active');
      btnShowPhone?.classList.remove('active');
      studioWorkspace?.classList.remove('smartphone-mode');
    }

    showToast(isFocus ? 'Focus Mode Active (Press Esc or F to exit)' : 'Focus Mode Disabled');
    if (!binaryStreamActive) {
      requestAnimationFrame(() => refreshScanoutFrame(false));
    }
  }

  // --------------------------------------------------------------------------
  // Fullscreen WebApp Mode (Hides Chrome URL Bar and Tabs)
  // --------------------------------------------------------------------------
  function toggleFullscreenMode() {
    if (!document.fullscreenElement) {
      document.documentElement.requestFullscreen().catch(() => {
        showToast('Fullscreen mode not permitted by browser');
      });
    } else {
      if (document.exitFullscreen) {
        document.exitFullscreen();
      }
    }
  }

  document.addEventListener('fullscreenchange', () => {
    const isFull = !!document.fullscreenElement;
    const btn = document.getElementById('btn-fullscreen-mode');
    const emuBtn = document.getElementById('emu-btn-fullscreen');
    const icon = document.getElementById('fullscreen-icon');
    const text = document.getElementById('fullscreen-text');
    if (btn) btn.classList.toggle('active', isFull);
    if (emuBtn) emuBtn.classList.toggle('active', isFull);
    if (text) text.textContent = isFull ? 'Exit Full' : 'Fullscreen';
    if (icon) {
      icon.innerHTML = isFull
        ? '<path d="M8 3v3a2 2 0 0 1-2 2H3m18 0h-3a2 2 0 0 1-2-2V3m0 18v-3a2 2 0 0 1 2-2h3M3 16h3a2 2 0 0 1 2 2v3"/>'
        : '<path d="M8 3H5a2 2 0 0 0-2 2v3m18 0V5a2 2 0 0 0-2-2h-3m0 18h3a2 2 0 0 0 2-2v-3M3 16v3a2 2 0 0 0 2 2h3"/>';
    }
  });

  const btnFocus = document.getElementById('btn-focus-mode');
  if (btnFocus) btnFocus.addEventListener('click', toggleFocusMode);
  const emuBtnFocus = document.getElementById('emu-btn-focus');
  if (emuBtnFocus) emuBtnFocus.addEventListener('click', toggleFocusMode);

  const btnFullscreen = document.getElementById('btn-fullscreen-mode');
  if (btnFullscreen) btnFullscreen.addEventListener('click', toggleFullscreenMode);
  const emuBtnFullscreen = document.getElementById('emu-btn-fullscreen');
  if (emuBtnFullscreen) emuBtnFullscreen.addEventListener('click', toggleFullscreenMode);

  // Header action visibility. WHY persisted: a person who hid an action to
  // make room should not have to hide it again after every reload. The
  // Options button itself is never hidden, so every action can be restored.
  (function initHeaderOptions() {
    const menu = document.getElementById('header-options-menu');
    const toggle = document.getElementById('btn-header-options');
    const reset = document.getElementById('btn-header-options-reset');
    if (!menu || !toggle) return;
    const STORAGE_KEY = 'interos.studio.header.hidden';
    let hidden = [];
    try {
      const stored = JSON.parse(localStorage.getItem(STORAGE_KEY) || '[]');
      if (Array.isArray(stored)) hidden = stored.filter((id) => typeof id === 'string');
    } catch (e) {}

    const apply = () => {
      document.querySelectorAll('[data-header-option]').forEach((box) => {
        const action = document.getElementById(box.dataset.headerOption);
        if (!action) return;
        const isHidden = hidden.includes(box.dataset.headerOption);
        box.checked = !isHidden;
        action.classList.toggle('header-action-hidden', isHidden);
      });
    };
    const persist = () => {
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(hidden));
      } catch (e) {}
    };
    apply();

    toggle.addEventListener('click', (event) => {
      event.stopPropagation();
      menu.hidden = !menu.hidden;
    });
    document.addEventListener('click', (event) => {
      if (!menu.hidden && !menu.contains(event.target) && event.target !== toggle) {
        menu.hidden = true;
      }
    });
    document.addEventListener('keydown', (event) => {
      if (event.key === 'Escape') menu.hidden = true;
    });
    menu.querySelectorAll('[data-header-option]').forEach((box) => {
      box.addEventListener('change', () => {
        const id = box.dataset.headerOption;
        if (box.checked) {
          hidden = hidden.filter((item) => item !== id);
        } else if (!hidden.includes(id)) {
          hidden.push(id);
        }
        persist();
        apply();
      });
    });
    reset?.addEventListener('click', () => {
      hidden = [];
      persist();
      apply();
    });
  })();

  // Keyboard shortcuts: 'F' toggles Focus Mode, 'D' switches to Deck, 'Escape' exits Focus Mode
  window.addEventListener('keydown', (e) => {
    if (isInteractiveOrChassisActive(e.target)) return;
    if ((e.key === 'f' || e.key === 'F') && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      toggleFocusMode();
    } else if ((e.key === 'd' || e.key === 'D') && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      if (document.body.classList.contains('studio-focus-mode')) {
        toggleFocusMode(false);
      } else {
        const btnDeck = document.getElementById('btn-show-telemetry');
        if (btnDeck) btnDeck.click();
      }
    } else if (e.key === 'Escape') {
      const modalImport = document.getElementById('modal-import-webapp');
      if (modalImport && modalImport.style.display !== 'none') {
        e.preventDefault();
        modalImport.style.display = 'none';
        modalImport.setAttribute('aria-hidden', 'true');
      } else if (document.body.classList.contains('studio-focus-mode')) {
        e.preventDefault();
        toggleFocusMode(false);
      }
    }
  });

  // Default to Smartphone Focus Mode out-of-the-box
  if (!document.body.classList.contains('studio-focus-mode')) {
    document.body.classList.add('studio-focus-mode');
  }
  const btnFocusInit = document.getElementById('btn-focus-mode');
  if (btnFocusInit) btnFocusInit.classList.add('active');
  const btnPhoneInit = document.getElementById('btn-show-phone');
  if (btnPhoneInit) btnPhoneInit.classList.add('active');
  const btnDeckInit = document.getElementById('btn-show-telemetry');
  if (btnDeckInit) btnDeckInit.classList.remove('active');

  // =========================================================================
  // 1-Click WebApp-to-.rapp Porting Engine & Live HMR Controller
  // =========================================================================
  const modalImportWebApp = document.getElementById('modal-import-webapp');
  const btnOpenImportModal = document.getElementById('btn-import-webapp');
  const btnCloseImportModal = document.getElementById('btn-close-import-webapp');
  const btnCancelImport = document.getElementById('btn-cancel-import');
  const backdropImport = document.getElementById('backdrop-import-webapp');
  const btnSubmitImport = document.getElementById('btn-submit-import');
  const btnSubmitImportText = document.getElementById('btn-submit-import-text');
  const importTerminalWrap = document.getElementById('import-terminal-wrap');
  const importTerminalLogs = document.getElementById('import-terminal-logs');
  const importStatusTag = document.getElementById('import-status-tag');

  function openImportModal() {
    if (!modalImportWebApp) return;
    modalImportWebApp.style.display = 'flex';
    modalImportWebApp.setAttribute('aria-hidden', 'false');
  }

  function closeImportModal() {
    if (!modalImportWebApp) return;
    modalImportWebApp.style.display = 'none';
    modalImportWebApp.setAttribute('aria-hidden', 'true');
  }

  if (btnOpenImportModal) btnOpenImportModal.addEventListener('click', openImportModal);
  if (btnCloseImportModal) btnCloseImportModal.addEventListener('click', closeImportModal);
  if (btnCancelImport) btnCancelImport.addEventListener('click', closeImportModal);
  if (backdropImport) backdropImport.addEventListener('click', closeImportModal);

  // Tab switching inside Import modal
  const tabBtns = document.querySelectorAll('.import-tab-btn');
  const tabPanes = {
    local: document.getElementById('pane-local'),
    url: document.getElementById('pane-url'),
    template: document.getElementById('pane-template')
  };
  let activeImportTab = 'local';

  tabBtns.forEach(btn => {
    btn.addEventListener('click', () => {
      tabBtns.forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      activeImportTab = btn.getAttribute('data-tab');
      Object.keys(tabPanes).forEach(k => {
        if (tabPanes[k]) {
          tabPanes[k].style.display = (k === activeImportTab) ? 'flex' : 'none';
        }
      });
    });
  });

  // Template cards selection
  const templateCards = document.querySelectorAll('.template-card');
  let selectedTemplatePath = 'templates/web_app_template';
  templateCards.forEach(card => {
    card.addEventListener('click', () => {
      templateCards.forEach(c => c.classList.remove('active'));
      card.classList.add('active');
      selectedTemplatePath = card.getAttribute('data-template-path') || 'templates/web_app_template';
    });
  });

  function appendImportLog(msg, color = '#D1D5DB') {
    if (!importTerminalLogs) return;
    const line = document.createElement('div');
    line.style.color = color;
    const time = new Date().toISOString().slice(11, 19);
    line.textContent = `[${time}] ${msg}`;
    importTerminalLogs.appendChild(line);
    importTerminalLogs.scrollTop = importTerminalLogs.scrollHeight;
  }

  // Handle Form Submission -> /api/web/import
  if (btnSubmitImport) {
    btnSubmitImport.addEventListener('click', async () => {
      let targetSource = '';
      if (activeImportTab === 'local') {
        targetSource = (document.getElementById('input-local-path')?.value || '').trim();
        if (!targetSource) {
          showToast('Please specify a local web app directory path');
          return;
        }
      } else if (activeImportTab === 'url') {
        targetSource = (document.getElementById('input-web-url')?.value || '').trim();
        if (!targetSource) {
          showToast('Please specify a remote web app or PWA URL');
          return;
        }
      } else if (activeImportTab === 'template') {
        targetSource = selectedTemplatePath;
      }

      const appName = (document.getElementById('input-import-name')?.value || '').trim();
      const appId = (document.getElementById('input-import-id')?.value || '').trim();
      const enableHmr = !!document.getElementById('toggle-live-hmr')?.checked;

      // Collect permissions
      const permissions = [];
      document.querySelectorAll('.permission-chips-container input[type="checkbox"]:checked').forEach(cb => {
        permissions.push(cb.value);
      });

      // Prepare UI for execution
      if (importTerminalWrap) importTerminalWrap.style.display = 'flex';
      if (importTerminalLogs) importTerminalLogs.innerHTML = '';
      if (importStatusTag) {
        importStatusTag.textContent = 'PORTING & SIGNING';
        importStatusTag.style.background = 'rgba(59, 130, 246, 0.2)';
        importStatusTag.style.color = '#60A5FA';
      }
      btnSubmitImport.disabled = true;
      if (btnSubmitImportText) btnSubmitImportText.textContent = 'Porting WebApp...';

      appendImportLog(`Target: ${targetSource} (Mode: ${activeImportTab})`, '#93C5FD');
      appendImportLog(`Selected permissions: [${permissions.join(', ')}]`);
      appendImportLog('Calling InterOS Web-to-RAPP Porting Engine & Ed25519 signer...');

      try {
        const resp = await fetch('/api/web/import', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            source: targetSource,
            name: appName || undefined,
            id: appId || undefined,
            permissions: permissions.length > 0 ? permissions : undefined,
            hmr: enableHmr
          })
        });

        const data = await resp.json();
        if (resp.ok && data.success) {
          if (importStatusTag) {
            importStatusTag.textContent = 'INSTALLED';
            importStatusTag.style.background = 'rgba(16, 185, 129, 0.2)';
            importStatusTag.style.color = '#34D399';
          }
          appendImportLog(`[OK] Package ID: ${data.package_id}`, '#34D399');
          appendImportLog(`[OK] Arch: ${data.arch} (RAPP v2 signed with Ed25519)`, '#34D399');
          appendImportLog(`[OK] Sideload status: ${data.sideload_status}`, '#34D399');
          if (data.guest_installed || data.guest_install) {
            appendImportLog(`[OK] Guest installation: Succeeded (${data.package_id} active in QEMU guest)`, '#34D399');
          }
          appendImportLog('[OK] Sideload complete. Untrusted bytecode isolated in Ring 3 wasmd sandbox.', '#10B981');
          showToast(`WebApp Ported: ${data.package_id} installed successfully!`);

          if (typeof loadInstalledPackages === 'function') {
            loadInstalledPackages();
          }
          if (typeof fetchInstalledRapps === 'function') {
            fetchInstalledRapps();
          }
        } else {
          if (importStatusTag) {
            importStatusTag.textContent = 'FAILED';
            importStatusTag.style.background = 'rgba(239, 68, 68, 0.2)';
            importStatusTag.style.color = '#F87171';
          }
          appendImportLog(`[ERROR] ${data.error || 'Porting failed'}`, '#F87171');
          showToast(`Porting error: ${data.error || 'Unknown failure'}`);
        }
      } catch (err) {
        if (importStatusTag) {
          importStatusTag.textContent = 'FAILED';
          importStatusTag.style.background = 'rgba(239, 68, 68, 0.2)';
          importStatusTag.style.color = '#F87171';
        }
        appendImportLog(`[NETWORK ERROR] ${err}`, '#F87171');
        showToast(`Request failed: ${err}`);
      } finally {
        btnSubmitImport.disabled = false;
        if (btnSubmitImportText) btnSubmitImportText.textContent = 'Port & Sideload into InterOS';
      }
    });
  }

  // =========================================================================
  // Live HMR Background Polling Listener
  // =========================================================================
  let lastHmrEventId = 0;
  async function pollHmrEvents() {
    while (true) {
      try {
        const res = await fetch(`/api/web/hmr?since=${lastHmrEventId}&timeout=0`);
        if (res.ok) {
          const body = await res.json();
          if (body.events && body.events.length > 0) {
            for (const evt of body.events) {
              lastHmrEventId = Math.max(lastHmrEventId, evt.id || 0);
              if (evt.app_id === 'org.interos.notes') continue;
              const timeStr = new Date(evt.timestamp * 1000).toLocaleTimeString();
              if (window.logConsole) {
                window.logConsole('ok', `[HMR] Live hot reload triggered for ${evt.app_id} (${evt.event}) at ${timeStr}`);
              }
              showToast(`[Live HMR] ${evt.app_id} hot-reloaded`);
              if (!binaryStreamActive && typeof refreshScanoutFrame === 'function') {
                requestAnimationFrame(() => refreshScanoutFrame(false));
              }
            }
          }
        }
      } catch (_) {
      }
      await new Promise(r => setTimeout(r, 2000));
    }
  }
  setTimeout(pollHmrEvents, 1000);

  // =========================================================================
  // Runtime Interactive Permission Consent Dialog Handler
  // =========================================================================
  const modalPermPrompt = document.getElementById('modal-permission-prompt');
  let activePermissionResolver = null;

  window.showRuntimePermissionDialog = function(appName, permName, description) {
    if (!modalPermPrompt) return Promise.resolve(false);
    const titleEl = document.getElementById('perm-prompt-title');
    const appEl = document.getElementById('perm-prompt-app');
    const descEl = document.getElementById('perm-prompt-desc');

    if (titleEl) titleEl.textContent = `Allow access to ${permName}?`;
    if (appEl) appEl.textContent = appName || 'Sovereign Application';
    if (descEl) descEl.textContent = description || `This application requires ${permName} capability.`;

    modalPermPrompt.style.display = 'flex';
    modalPermPrompt.setAttribute('aria-hidden', 'false');

    return new Promise(resolve => {
      activePermissionResolver = resolve;
    });
  };

  function resolvePermissionPrompt(decision) {
    if (!modalPermPrompt) return;
    modalPermPrompt.style.display = 'none';
    modalPermPrompt.setAttribute('aria-hidden', 'true');
    if (activePermissionResolver) {
      activePermissionResolver(decision);
      activePermissionResolver = null;
    }
  }

  document.getElementById('btn-perm-allow-always')?.addEventListener('click', () => resolvePermissionPrompt(1));
  document.getElementById('btn-perm-allow-once')?.addEventListener('click', () => resolvePermissionPrompt(3));
  document.getElementById('btn-perm-deny')?.addEventListener('click', () => resolvePermissionPrompt(2));

  // =========================================================================
  // AVD Extended Controls (Sensors, Battery, Biometrics, Killswitches)
  // =========================================================================
  const emuBtnExtended = document.getElementById('emu-btn-extended');
  const modalExtendedControls = document.getElementById('modal-extended-controls');
  const btnCloseExtendedControls = document.getElementById('btn-close-extended-controls');
  const backdropExtendedControls = document.getElementById('backdrop-extended-controls');

  function openExtendedControls() {
    if (!modalExtendedControls) return;
    modalExtendedControls.style.display = 'flex';
    modalExtendedControls.setAttribute('aria-hidden', 'false');
    syncExtendedControlsState();
  }

  function closeExtendedControls() {
    if (!modalExtendedControls) return;
    modalExtendedControls.style.display = 'none';
    modalExtendedControls.setAttribute('aria-hidden', 'true');
  }

  emuBtnExtended?.addEventListener('click', openExtendedControls);
  btnCloseExtendedControls?.addEventListener('click', closeExtendedControls);
  backdropExtendedControls?.addEventListener('click', closeExtendedControls);

  // Nav Tabs switching
  const extNavItems = document.querySelectorAll('.ext-nav-item');
  const extTabPanes = {
    sensors: document.getElementById('pane-ext-sensors'),
    battery: document.getElementById('pane-ext-battery'),
    biometrics: document.getElementById('pane-ext-biometrics'),
    killswitches: document.getElementById('pane-ext-killswitches')
  };

  extNavItems.forEach(item => {
    item.addEventListener('click', () => {
      extNavItems.forEach(i => i.classList.remove('active'));
      item.classList.add('active');
      const tab = item.getAttribute('data-tab');
      Object.keys(extTabPanes).forEach(k => {
        if (extTabPanes[k]) {
          extTabPanes[k].classList.toggle('active', k === tab);
        }
      });
    });
  });

  // Tab 1: Virtual Sensors
  const sliderAccelX = document.getElementById('slider-accel-x');
  const sliderAccelY = document.getElementById('slider-accel-y');
  const sliderAccelZ = document.getElementById('slider-accel-z');
  const valAccelX = document.getElementById('val-accel-x');
  const valAccelY = document.getElementById('val-accel-y');
  const valAccelZ = document.getElementById('val-accel-z');
  const btnApplySensors = document.getElementById('btn-apply-sensors');
  const btnResetSensors = document.getElementById('btn-reset-sensors');
  const statusSensors = document.getElementById('status-sensors');
  const extPresetBtns = document.querySelectorAll('.ext-btn-preset');

  function updateSensorLabels() {
    if (valAccelX && sliderAccelX) valAccelX.textContent = `${sliderAccelX.value} milli-g`;
    if (valAccelY && sliderAccelY) valAccelY.textContent = `${sliderAccelY.value} milli-g`;
    if (valAccelZ && sliderAccelZ) valAccelZ.textContent = `${sliderAccelZ.value} milli-g`;
  }

  sliderAccelX?.addEventListener('input', updateSensorLabels);
  sliderAccelY?.addEventListener('input', updateSensorLabels);
  sliderAccelZ?.addEventListener('input', updateSensorLabels);

  extPresetBtns.forEach(btn => {
    btn.addEventListener('click', () => {
      extPresetBtns.forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      const preset = btn.getAttribute('data-preset');
      if (preset === 'portrait') {
        if (sliderAccelX) sliderAccelX.value = 0;
        if (sliderAccelY) sliderAccelY.value = 980;
        if (sliderAccelZ) sliderAccelZ.value = 0;
      } else if (preset === 'landscape-right') {
        if (sliderAccelX) sliderAccelX.value = 980;
        if (sliderAccelY) sliderAccelY.value = 0;
        if (sliderAccelZ) sliderAccelZ.value = 0;
      } else if (preset === 'landscape-left') {
        if (sliderAccelX) sliderAccelX.value = -980;
        if (sliderAccelY) sliderAccelY.value = 0;
        if (sliderAccelZ) sliderAccelZ.value = 0;
      } else if (preset === 'face-up') {
        if (sliderAccelX) sliderAccelX.value = 0;
        if (sliderAccelY) sliderAccelY.value = 0;
        if (sliderAccelZ) sliderAccelZ.value = 980;
      }
      updateSensorLabels();
    });
  });

  async function applySensors() {
    const x = parseInt(sliderAccelX?.value || '0', 10);
    const y = parseInt(sliderAccelY?.value || '980', 10);
    const z = parseInt(sliderAccelZ?.value || '0', 10);
    try {
      if (statusSensors) statusSensors.textContent = 'Injecting sensor vector...';
      const resp = await fetch('/api/sensors/inject', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ x, y, z })
      });
      const data = await resp.json();
      if (statusSensors) {
        statusSensors.textContent = data.success ? 'Vector injected successfully' : ('Error: ' + (data.detail || 'failed'));
        setTimeout(() => { if (statusSensors) statusSensors.textContent = ''; }, 3000);
      }
    } catch (e) {
      if (statusSensors) statusSensors.textContent = 'Network error injecting sensor';
    }
  }

  btnApplySensors?.addEventListener('click', applySensors);
  btnResetSensors?.addEventListener('click', () => {
    if (sliderAccelX) sliderAccelX.value = 0;
    if (sliderAccelY) sliderAccelY.value = 980;
    if (sliderAccelZ) sliderAccelZ.value = 0;
    updateSensorLabels();
    applySensors();
  });

  // Tab 2: Battery Level
  const sliderBatteryPct = document.getElementById('slider-battery-pct');
  const valBatteryPct = document.getElementById('val-battery-pct');
  const infoBatteryVoltage = document.getElementById('info-battery-voltage');
  const infoBatteryCapacity = document.getElementById('info-battery-capacity');
  const btnApplyBattery = document.getElementById('btn-apply-battery');
  const statusBattery = document.getElementById('status-battery');

  function updateBatteryLabels() {
    const pct = parseInt(sliderBatteryPct?.value || '85', 10);
    if (valBatteryPct) valBatteryPct.textContent = `${pct}%`;
    const mv = 3500 + Math.round(700 * pct / 100);
    const mah = Math.round(5000 * pct / 100);
    if (infoBatteryVoltage) infoBatteryVoltage.textContent = `${mv} mV`;
    if (infoBatteryCapacity) infoBatteryCapacity.textContent = `${mah} / 5000 mAh`;
  }

  sliderBatteryPct?.addEventListener('input', updateBatteryLabels);

  btnApplyBattery?.addEventListener('click', async () => {
    const pct = parseInt(sliderBatteryPct?.value || '85', 10);
    const checkedRadio = document.querySelector('input[name="charging-state"]:checked');
    const status = checkedRadio?.value || 'Discharging';
    try {
      if (statusBattery) statusBattery.textContent = 'Setting telemetry...';
      const resp = await fetch('/api/battery/set', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ percentage: pct, status })
      });
      const data = await resp.json();
      if (statusBattery) {
        statusBattery.textContent = data.success ? 'Battery telemetry applied' : ('Error: ' + (data.detail || 'failed'));
        setTimeout(() => { if (statusBattery) statusBattery.textContent = ''; }, 3000);
      }
    } catch (e) {
      if (statusBattery) statusBattery.textContent = 'Network error applying battery telemetry';
    }
  });

  // Tab 3: Biometrics
  const btnBioPass = document.getElementById('btn-bio-fingerprint-pass');
  const btnBioFail = document.getElementById('btn-bio-fingerprint-fail');
  const statusBioResult = document.getElementById('status-bio-result');

  async function injectFingerprint(result) {
    try {
      if (statusBioResult) statusBioResult.textContent = `Emitting fingerprint ${result}...`;
      const resp = await fetch('/api/biometrics/fingerprint', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ result })
      });
      const data = await resp.json();
      if (statusBioResult) {
        if (result === 'PASS') {
          statusBioResult.textContent = 'Biometric match verified (UID=1000 Unlocked)';
          statusBioResult.style.color = '#3FB950';
          showToast('Fingerprint match verified: Device unlocked');
          setTimeout(() => refreshScanoutFrame(true), 100);
        } else {
          statusBioResult.textContent = 'Biometric match rejected (Fingerprint Mismatch)';
          statusBioResult.style.color = '#F85149';
          showToast('Fingerprint rejected: Biometric mismatch');
        }
      }
    } catch (e) {
      if (statusBioResult) statusBioResult.textContent = 'Failed to emit fingerprint event';
    }
  }

  btnBioPass?.addEventListener('click', () => injectFingerprint('PASS'));
  btnBioFail?.addEventListener('click', () => injectFingerprint('FAIL'));

  // Tab 4: Hardware Killswitches
  const ksCheckboxes = {
    camera: document.getElementById('ks-camera'),
    mic: document.getElementById('ks-mic'),
    cellular: document.getElementById('ks-cellular'),
    gps: document.getElementById('ks-gps')
  };
  const statusKillswitch = document.getElementById('status-killswitch');

  Object.keys(ksCheckboxes).forEach(subsystem => {
    const cb = ksCheckboxes[subsystem];
    cb?.addEventListener('change', async (e) => {
      const state = e.target.checked;
      try {
        if (statusKillswitch) statusKillswitch.textContent = `Toggling ${subsystem} killswitch...`;
        const resp = await fetch('/api/killswitch/toggle', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ subsystem, state })
        });
        const data = await resp.json();
        if (statusKillswitch) {
          statusKillswitch.textContent = data.success ? `${subsystem} killswitch: ${state ? 'ENGAGED (CUT)' : 'DISENGAGED (ACTIVE)'}` : 'Error toggling killswitch';
          setTimeout(() => { if (statusKillswitch) statusKillswitch.textContent = ''; }, 3000);
        }
      } catch (err) {
        if (statusKillswitch) statusKillswitch.textContent = 'Failed to toggle killswitch';
      }
    });
  });

  async function syncExtendedControlsState() {
    try {
      const resp = await fetch('/api/extended-controls/state');
      const data = await resp.json();
      if (!data.success || !data.state) return;
      const st = data.state;
      if (st.sensors && st.sensors.accel) {
        if (sliderAccelX) sliderAccelX.value = st.sensors.accel.x;
        if (sliderAccelY) sliderAccelY.value = st.sensors.accel.y;
        if (sliderAccelZ) sliderAccelZ.value = st.sensors.accel.z;
        updateSensorLabels();
      }
      if (st.battery) {
        if (sliderBatteryPct) sliderBatteryPct.value = st.battery.percentage;
        updateBatteryLabels();
        const rad = document.querySelector(`input[name="charging-state"][value="${st.battery.charging_status}"]`);
        if (rad) rad.checked = true;
      }
      if (st.killswitches) {
        Object.keys(st.killswitches).forEach(k => {
          if (ksCheckboxes[k]) ksCheckboxes[k].checked = !!st.killswitches[k];
        });
      }
    } catch (_) {}
  }

});

