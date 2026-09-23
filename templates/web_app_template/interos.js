/**
 * InterOS Universal Client SDK (interos.js)
 * 
 * Standard lightweight JavaScript API bridge for developers building applications
 * on Inter OS without needing Rust knowledge.
 * 
 * Provides capability-scoped access to:
 * - VFS storage (reading, writing, deleting sandboxed app files)
 * - Telemetry (battery fuel gauge, SoC thermals, IMU sensors)
 * - System audio & haptic feedback
 * - Notifications & toast alerts
 */

(function (window) {
  'use strict';

  const InterOS = {
    version: '8.2.0',
    platform: 'InterOS Sovereign Microkernel',

    /**
     * Capability-Governed Virtual File System (VFS)
     */
    vfs: {
      async list(path = '/') {
        const res = await fetch(`/api/fs/list?path=${encodeURIComponent(path)}`);
        return await res.json();
      },

      async readFile(path) {
        const res = await fetch('/api/fs/read', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path })
        });
        const data = await res.json();
        if (!data.success) throw new Error(data.error || 'Failed to read file');
        return data.content;
      },

      async writeFile(path, content) {
        const res = await fetch('/api/fs/write', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ path, content })
        });
        const data = await res.json();
        if (!data.success) throw new Error(data.error || 'Failed to write file');
        return data.bytes_written;
      },

      async deleteFile(path) {
        const res = await fetch('/api/fs/delete', {
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
        const res = await fetch('/api/qemu/telemetry');
        return await res.json();
      },

      async getBattery() {
        const t = await this.getStatus();
        return {
          level: t.battery_level || 94,
          voltageMv: t.battery_voltage_mv || 3850,
          charging: t.battery_charging || false
        };
      },

      async getThermals() {
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
        try {
          const ctx = new (window.AudioContext || window.webkitAudioContext)();
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
        } catch (e) {
          // Audio context suppressed
        }
      }
    },

    /**
     * User Notifications & Toast Alerts
     */
    notifications: {
      show(message, type = 'info') {
        if (typeof window.showToast === 'function') {
          window.showToast(message);
        } else {
          console.log(`[InterOS Notification] (${type}) ${message}`);
        }
      }
    }
  };

  window.InterOS = InterOS;
})(window);
