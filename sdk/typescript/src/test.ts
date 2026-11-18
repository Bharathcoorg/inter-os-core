// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

/**
 * Regression tests for the TypeScript RAPP builder's web-permission mapping.
 *
 * Run with `npm test` (node --test dist/test.js) after `npm run build`, or
 * directly on Node >= 23.6 with `node --test sdk/typescript/src/test.ts`.
 *
 * The bug these pin: MapWebPermission returned camera -> [4], microphone -> [7]
 * and geolocation -> [8]. Those are CONSENT-broker ids. In the RAPP v2 manifest
 * id space the kernel mints 4 as HardwarePortIo (READ|WRITE over object 0),
 * 7 as ProcessSpawn and 8 as ProcessKill (kernel/src/task/user.rs:747,750,751),
 * so an ordinary camera prompt handed a web app raw x86 port I/O and process
 * control. Unrecognized strings fell through to a default that granted
 * NetworkSocket + NetworkSend + NetworkReceive: ambient authority for a typo.
 *
 * tests/test_permission_mapping_contract.py additionally cross-checks this table
 * against sdk/go/rapp/builder.go, tools/port_web.py and the kernel source.
 */

import test from "node:test";
import assert from "node:assert/strict";

import {
  ArchWasm,
  ArchWeb,
  ManifestCapability,
  MANIFEST_CAPABILITY_MIN,
  MANIFEST_CAPABILITY_MAX,
  SensitivePermission,
  isManifestCapability,
  mapWebPermissionToCapabilities,
  mapWebPermissionToConsent,
  resolveWebPermission,
  RappBuilder,
} from "./builder.ts";

/** Manifest ids the kernel mints as raw hardware / process authority. */
const FORBIDDEN_FOR_WEB_PROMPTS = [
  ManifestCapability.HardwarePortIo, // 4
  ManifestCapability.ProcessSpawn, // 7
  ManifestCapability.ProcessKill, // 8
];

/** The corrected table, written out independently of the implementation. */
const EXPECTED: Readonly<Record<string, { manifest: number[]; consent: number[] }>> = {
  camera: { manifest: [22], consent: [4] },
  cameracapture: { manifest: [22], consent: [4] },
  microphone: { manifest: [21], consent: [7] },
  mic: { manifest: [21], consent: [7] },
  sensor: { manifest: [23], consent: [12] },
  sensors: { manifest: [23], consent: [12] },
  accelerometer: { manifest: [23], consent: [12] },
  gyroscope: { manifest: [23], consent: [12] },
  geolocation: { manifest: [], consent: [8] },
  location: { manifest: [], consent: [8] },
  notifications: { manifest: [], consent: [14] },
  notification: { manifest: [], consent: [14] },
  storage: { manifest: [9, 10], consent: [10] },
  "persistent-storage": { manifest: [9, 10], consent: [10] },
  files: { manifest: [9, 10], consent: [10] },
  filesystemread: { manifest: [9], consent: [] },
  "vfs.read": { manifest: [9], consent: [] },
  filesystemwrite: { manifest: [10], consent: [] },
  "vfs.write": { manifest: [10], consent: [] },
  network: { manifest: [12, 13, 14], consent: [3] },
  networksocket: { manifest: [12], consent: [] },
  networksend: { manifest: [13], consent: [] },
  networkreceive: { manifest: [14], consent: [] },
  audio: { manifest: [28], consent: [5] },
  "audio.feedback": { manifest: [28], consent: [5] },
  audiosynthesis: { manifest: [28], consent: [5] },
  telephony: { manifest: [20], consent: [1] },
  telephonymodem: { manifest: [20], consent: [1] },
  phone: { manifest: [20], consent: [1] },
  sms: { manifest: [20], consent: [1] },
  telemetry: { manifest: [19], consent: [] },
  "telemetry.battery": { manifest: [19], consent: [] },
};

test("camera resolves to SystemControlCamera (22), never HardwarePortIo (4)", () => {
  assert.deepEqual(mapWebPermissionToCapabilities("camera"), [22]);
  assert.deepEqual(mapWebPermissionToConsent("camera"), [4]);
  assert.equal(ManifestCapability.SystemControlCamera, 22);
  assert.equal(ManifestCapability.HardwarePortIo, 4);
});

test("microphone resolves to SystemControlAudio (21), never ProcessSpawn (7)", () => {
  assert.deepEqual(mapWebPermissionToCapabilities("microphone"), [21]);
  assert.deepEqual(mapWebPermissionToConsent("microphone"), [7]);
});

test("sensor resolves to SystemControlSensor (23), never NetworkSocket (12)", () => {
  for (const perm of ["sensor", "sensors", "accelerometer", "gyroscope"]) {
    assert.deepEqual(mapWebPermissionToCapabilities(perm), [23], perm);
    assert.deepEqual(mapWebPermissionToConsent(perm), [12], perm);
  }
});

test("geolocation has no manifest id and requests Location consent (8)", () => {
  // The ladder defines no GNSS object; RealtimeClockRead (24) is not location.
  assert.deepEqual(mapWebPermissionToCapabilities("geolocation"), []);
  assert.deepEqual(mapWebPermissionToConsent("geolocation"), [8]);
  assert.equal(SensitivePermission.Location, 8);
});

test("notifications are consent-only, never NetworkReceive (14) or EventPoll (17)", () => {
  assert.deepEqual(mapWebPermissionToCapabilities("notifications"), []);
  assert.deepEqual(mapWebPermissionToConsent("notifications"), [14]);
});

test("the two id spaces are distinct even though 4/7/8 collide numerically", () => {
  assert.equal(ManifestCapability.HardwarePortIo, SensitivePermission.CameraCapture);
  assert.equal(ManifestCapability.ProcessSpawn, SensitivePermission.Microphone);
  assert.equal(ManifestCapability.ProcessKill, SensitivePermission.Location);
  assert.equal(ManifestCapability.NetworkSocket, SensitivePermission.Sensors);
  assert.equal(ManifestCapability.NetworkReceive, SensitivePermission.Notifications);
  // ...but they are different authorities, so a consent id must never be
  // emitted as a manifest id.
  assert.notEqual(ManifestCapability.SystemControlCamera, SensitivePermission.CameraCapture);
});

test("every known permission matches the corrected table exactly", () => {
  for (const [perm, want] of Object.entries(EXPECTED)) {
    const got = resolveWebPermission(perm);
    assert.deepEqual(got.capabilities, want.manifest, `${perm} manifest`);
    assert.deepEqual(got.consent, want.consent, `${perm} consent`);
  }
});

test("no web permission ever grants raw port I/O or process spawn/kill", () => {
  for (const perm of Object.keys(EXPECTED)) {
    const caps = mapWebPermissionToCapabilities(perm);
    for (const forbidden of FORBIDDEN_FOR_WEB_PROMPTS) {
      assert.ok(!caps.includes(forbidden), `${perm} granted forbidden manifest id ${forbidden}`);
    }
  }
});

test("permission lookup is case- and whitespace-insensitive", () => {
  assert.deepEqual(mapWebPermissionToCapabilities("  CAMERA  "), [22]);
  assert.deepEqual(mapWebPermissionToCapabilities("MicroPhone"), [21]);
  assert.deepEqual(mapWebPermissionToConsent("GeoLocation"), [8]);
});

test("an unknown permission throws instead of granting ambient network", () => {
  for (const unknown of [
    "", "   ", "admin", "root", "shell", "debugger", "camera;rm -rf /",
    "network-all", "*", "0", "31", "999", "not-a-real-permission",
  ]) {
    assert.throws(
      () => resolveWebPermission(unknown),
      /unknown web permission/,
      `unknown permission ${JSON.stringify(unknown)} did not fail closed`,
    );
    assert.throws(() => mapWebPermissionToCapabilities(unknown), /unknown web permission/);
    assert.throws(() => mapWebPermissionToConsent(unknown), /unknown web permission/);
  }
});

test("the manifest ladder covers 1..=30 and matches the kernel variant names", () => {
  assert.equal(MANIFEST_CAPABILITY_MIN, 1);
  assert.equal(MANIFEST_CAPABILITY_MAX, 30);
  const kernelLadder: Readonly<Record<string, number>> = {
    SerialOutput: 1, FrameBufferDirect: 2, TimerTickRead: 3, HardwarePortIo: 4,
    IpcSend: 5, IpcReceive: 6, ProcessSpawn: 7, ProcessKill: 8,
    FileSystemRead: 9, FileSystemWrite: 10, StorageRawBlock: 11, NetworkSocket: 12,
    NetworkSend: 13, NetworkReceive: 14, ShmProducer: 15, ShmConsumer: 16,
    EventPoll: 17, SystemControl: 18, SystemControlPower: 19, SystemControlTelephony: 20,
    SystemControlAudio: 21, SystemControlCamera: 22, SystemControlSensor: 23,
    RealtimeClockRead: 24, ContentIndexRead: 25, ContentIntentDispatch: 26,
    SurfaceCapture: 27, AudioStream: 28, MemoryHeap: 29, ProcessThreads: 30,
  };
  assert.deepEqual({ ...ManifestCapability }, kernelLadder);
  for (let id = 1; id <= 30; id++) assert.ok(isManifestCapability(id), `id ${id}`);
  for (const id of [0, -1, 31, 32, 39, 999, 1.5, NaN]) {
    assert.ok(!isManifestCapability(id), `id ${id} must be outside the ladder`);
  }
});

test("the consent table matches SensitivePermission::from_u32 (1..=20)", () => {
  const kernelConsent: Readonly<Record<string, number>> = {
    TelephonyModem: 1, StorageSectorCrypt: 2, NetworkSend: 3, CameraCapture: 4,
    AudioSynthesis: 5, RtcClock: 6, Microphone: 7, Location: 8, Contacts: 9,
    Files: 10, Sms: 11, Sensors: 12, Bluetooth: 13, Notifications: 14,
    AgentTask: 15, AgentToolCalculator: 16, AgentToolClock: 17, AgentToolStorage: 18,
    AgentToolSettings: 19, AgentToolCapCheck: 20,
  };
  assert.deepEqual({ ...SensitivePermission }, kernelConsent);
});

test("every granted id is inside the manifest ladder and the consent range", () => {
  for (const [perm, grant] of Object.entries(EXPECTED)) {
    for (const id of grant.manifest) {
      assert.ok(isManifestCapability(id), `${perm} manifest id ${id} outside 1..=30`);
    }
    for (const id of grant.consent) {
      assert.ok(Number.isInteger(id) && id >= 1 && id <= 20, `${perm} consent id ${id}`);
    }
  }
});

test("returned grants are copies, so a caller cannot corrupt the shared table", () => {
  const first = resolveWebPermission("camera");
  first.capabilities.push(4);
  first.consent.length = 0;
  assert.deepEqual(mapWebPermissionToCapabilities("camera"), [22]);
  assert.deepEqual(mapWebPermissionToConsent("camera"), [4]);
});

test("buildBinary refuses capability ids outside the manifest ladder", () => {
  const key = new Uint8Array(32).fill(7);
  for (const bad of [0, 31, 32, 39, 999]) {
    const b = RappBuilder.create("test.ladder")
      .setPublisherKey(key)
      .setArch(ArchWeb)
      .setPayload(new Uint8Array([1, 2, 3]))
      .addCapability(bad as never);
    assert.throws(() => b.buildBinary(), /outside the RAPP v2 manifest ladder/, `id ${bad}`);
  }
});

test("buildBinary emits the corrected ids in the header capability array", () => {
  const key = new Uint8Array(32).fill(7);
  const b = RappBuilder.create("test.webapp").setPublisherKey(key).setArch(ArchWeb);
  for (const perm of ["camera", "microphone", "sensor"]) {
    for (const cap of mapWebPermissionToCapabilities(perm)) b.addCapability(cap);
  }
  const out = b.setPayload(new Uint8Array([0x7f, 0x45, 0x4c, 0x46])).buildBinary();
  const view = new DataView(out.buffer, out.byteOffset, out.byteLength);
  assert.equal(view.getUint16(0x06, true), ArchWeb);
  assert.equal(ArchWasm, ArchWeb, "wasm and web share arch id 3");
  const count = view.getUint16(0x4e, true);
  assert.equal(count, 3);
  const caps: number[] = [];
  for (let i = 0; i < count; i++) caps.push(view.getUint16(0xb4 + i * 2, true));
  assert.deepEqual(caps.sort((x, y) => x - y), [21, 22, 23]);
  for (const forbidden of FORBIDDEN_FOR_WEB_PROMPTS) assert.ok(!caps.includes(forbidden));
  // Unsigned by default: the signature field stays zero until signing.
  assert.deepEqual(out.subarray(0x70, 0xb0), new Uint8Array(64));
});