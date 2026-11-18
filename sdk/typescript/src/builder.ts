// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

/**
 * Fluent builder for canonical RAPP v2 packages.
 */
import { CapabilityType, RappArch, type ResourceLimits } from "./types.ts";
import { sha256, signRappV2 } from "./crypto.ts";

export const ArchWasm = 3;
export const ArchWeb = 3;

/**
 * The complete RAPP v2 manifest capability ladder, ids 1..=30.
 *
 * Pinned to kernel/src/capability/mod.rs (CapabilityType::to_raw),
 * kernel/src/task/user.rs (capabilities_from_rapp_with_trust) and
 * docs/RAPP_FORMAT.md section 3. Package verification accepts 1..=30 only
 * (kernel/src/package/rapp.rs, parse_rapp); the service-only types 31..39 exist
 * in the kernel enum but are deliberately outside the manifest ABI.
 *
 * `./types.ts` still enumerates only 1..18, so the extended identifiers are
 * carried here and narrowed to `CapabilityType` at the mapper boundary.
 *
 * This is the MANIFEST id space. It is not the consent id space below: here 4 is
 * HardwarePortIo, 7 is ProcessSpawn and 8 is ProcessKill.
 */
export const ManifestCapability = {
  SerialOutput: 1,
  FrameBufferDirect: 2,
  TimerTickRead: 3,
  HardwarePortIo: 4,
  IpcSend: 5,
  IpcReceive: 6,
  ProcessSpawn: 7,
  ProcessKill: 8,
  FileSystemRead: 9,
  FileSystemWrite: 10,
  StorageRawBlock: 11,
  NetworkSocket: 12,
  NetworkSend: 13,
  NetworkReceive: 14,
  ShmProducer: 15,
  ShmConsumer: 16,
  EventPoll: 17,
  SystemControl: 18,
  // Domain-scoped SystemControl: INVOKE bound to one service object, so holding
  // one confers no authority over any other domain (user.rs:762-766).
  SystemControlPower: 19, // SERVICE_POWER_CONTROL_OBJECT: power governor, battery telemetry
  SystemControlTelephony: 20, // SERVICE_TELEPHONY_CONTROL_OBJECT: modem, SMS, dialer
  SystemControlAudio: 21, // SERVICE_AUDIO_CONTROL_OBJECT: audio control; the microphone grant
  SystemControlCamera: 22, // SERVICE_CAMERA_CONTROL_OBJECT: camera frame capture
  SystemControlSensor: 23, // SERVICE_SENSOR_CONTROL_OBJECT: IMU, accelerometer, biometrics
  RealtimeClockRead: 24, // rtc.read
  ContentIndexRead: 25, // content.index_read
  ContentIntentDispatch: 26, // content.intent_dispatch
  SurfaceCapture: 27, // surface.capture
  AudioStream: 28, // audio.stream: AC'97 PCM output
  MemoryHeap: 29, // memory.heap
  ProcessThreads: 30, // process.threads
} as const;

export type ManifestCapabilityId = (typeof ManifestCapability)[keyof typeof ManifestCapability];

export const MANIFEST_CAPABILITY_MIN = 1;
export const MANIFEST_CAPABILITY_MAX = 30;

/** True when id is inside the RAPP v2 manifest ladder the kernel will accept. */
export function isManifestCapability(id: number): boolean {
  return Number.isInteger(id) && id >= MANIFEST_CAPABILITY_MIN && id <= MANIFEST_CAPABILITY_MAX;
}

/**
 * The kernel's CONSENT-BROKER permission id space
 * (kernel/src/capability/permission.rs:37-85, SensitivePermission::from_u32).
 *
 * These are the values a user consent prompt grants at runtime; the broker mints
 * a scoped capability from them (permission.rs:189-293). They are never written
 * into a .rapp header. CameraCapture(4), Microphone(7) and Location(8) here are
 * the same numbers as HardwarePortIo(4), ProcessSpawn(7) and ProcessKill(8) in
 * the manifest space, which is why the two are kept distinct.
 *
 * Minted objects: Camera 0xCA00, Microphone 0xCA01, Location 0xCA02,
 * Sensors 0x5E45, Notifications IpcSend @ 0x9071.
 */
export const SensitivePermission = {
  TelephonyModem: 1,
  StorageSectorCrypt: 2,
  NetworkSend: 3,
  CameraCapture: 4,
  AudioSynthesis: 5,
  RtcClock: 6,
  Microphone: 7,
  Location: 8,
  Contacts: 9,
  Files: 10,
  Sms: 11,
  Sensors: 12,
  Bluetooth: 13,
  Notifications: 14,
  AgentTask: 15,
  AgentToolCalculator: 16,
  AgentToolClock: 17,
  AgentToolStorage: 18,
  AgentToolSettings: 19,
  AgentToolCapCheck: 20,
} as const;

export type SensitivePermissionId = (typeof SensitivePermission)[keyof typeof SensitivePermission];

/** One web permission resolved into both kernel id spaces. */
export interface WebPermissionGrant {
  /** RAPP v2 manifest ids for the package header. May be empty. */
  capabilities: CapabilityType[];
  /** Consent-broker ids the user must approve at runtime. May be empty. */
  consent: SensitivePermissionId[];
}

interface RawWebPermissionGrant {
  manifest: readonly number[];
  consent: readonly number[];
}

/**
 * The single authoritative web-permission table, mirrored by
 * sdk/go/rapp/builder.go and tools/port_web.py.
 * tests/test_permission_mapping_contract.py pins all three against each other
 * and against the kernel.
 *
 * WHY camera is not 4, microphone not 7 and geolocation not 8: those are the
 * CONSENT ids. In the kernel raw type space the same numbers are HardwarePortIo,
 * ProcessSpawn and ProcessKill (kernel/src/capability/mod.rs,
 * CapabilityType::to_raw), and the manifest minter used to pass them straight
 * through, so emitting them for a camera/mic/location prompt handed a web app
 * raw x86 port I/O and process control. The minter now refuses 4/7/8 without
 * system platform trust and reshapes them through sensitive_manifest_shape, but
 * the scoped service grants above remain the correct declaration.
 *
 * geolocation and notifications have NO manifest identifier: the ladder defines
 * no GNSS object, and NotificationPost (syscall 110) authorizes on the caller's
 * own package identity. kernel/src/package/dependency.rs
 * (from_rapp_capabilities) derives neither Permission::Location nor
 * Permission::Notifications from any manifest id. Their only honest grant is a
 * consent request, so their manifest list is empty.
 */
const WEB_PERMISSION_GRANTS: Readonly<Record<string, RawWebPermissionGrant>> = {
  camera: { manifest: [ManifestCapability.SystemControlCamera], consent: [SensitivePermission.CameraCapture] },
  cameracapture: { manifest: [ManifestCapability.SystemControlCamera], consent: [SensitivePermission.CameraCapture] },

  microphone: { manifest: [ManifestCapability.SystemControlAudio], consent: [SensitivePermission.Microphone] },
  mic: { manifest: [ManifestCapability.SystemControlAudio], consent: [SensitivePermission.Microphone] },

  sensor: { manifest: [ManifestCapability.SystemControlSensor], consent: [SensitivePermission.Sensors] },
  sensors: { manifest: [ManifestCapability.SystemControlSensor], consent: [SensitivePermission.Sensors] },
  accelerometer: { manifest: [ManifestCapability.SystemControlSensor], consent: [SensitivePermission.Sensors] },
  gyroscope: { manifest: [ManifestCapability.SystemControlSensor], consent: [SensitivePermission.Sensors] },

  geolocation: { manifest: [], consent: [SensitivePermission.Location] },
  location: { manifest: [], consent: [SensitivePermission.Location] },

  notifications: { manifest: [], consent: [SensitivePermission.Notifications] },
  notification: { manifest: [], consent: [SensitivePermission.Notifications] },

  storage: { manifest: [ManifestCapability.FileSystemRead, ManifestCapability.FileSystemWrite], consent: [SensitivePermission.Files] },
  "persistent-storage": { manifest: [ManifestCapability.FileSystemRead, ManifestCapability.FileSystemWrite], consent: [SensitivePermission.Files] },
  files: { manifest: [ManifestCapability.FileSystemRead, ManifestCapability.FileSystemWrite], consent: [SensitivePermission.Files] },
  filesystemread: { manifest: [ManifestCapability.FileSystemRead], consent: [] },
  "vfs.read": { manifest: [ManifestCapability.FileSystemRead], consent: [] },
  filesystemwrite: { manifest: [ManifestCapability.FileSystemWrite], consent: [] },
  "vfs.write": { manifest: [ManifestCapability.FileSystemWrite], consent: [] },

  // Network is granted only when asked for by name, never as a fallback.
  network: {
    manifest: [ManifestCapability.NetworkSocket, ManifestCapability.NetworkSend, ManifestCapability.NetworkReceive],
    consent: [SensitivePermission.NetworkSend],
  },
  networksocket: { manifest: [ManifestCapability.NetworkSocket], consent: [] },
  networksend: { manifest: [ManifestCapability.NetworkSend], consent: [] },
  networkreceive: { manifest: [ManifestCapability.NetworkReceive], consent: [] },

  audio: { manifest: [ManifestCapability.AudioStream], consent: [SensitivePermission.AudioSynthesis] },
  "audio.feedback": { manifest: [ManifestCapability.AudioStream], consent: [SensitivePermission.AudioSynthesis] },
  audiosynthesis: { manifest: [ManifestCapability.AudioStream], consent: [SensitivePermission.AudioSynthesis] },

  telephony: { manifest: [ManifestCapability.SystemControlTelephony], consent: [SensitivePermission.TelephonyModem] },
  telephonymodem: { manifest: [ManifestCapability.SystemControlTelephony], consent: [SensitivePermission.TelephonyModem] },
  phone: { manifest: [ManifestCapability.SystemControlTelephony], consent: [SensitivePermission.TelephonyModem] },
  sms: { manifest: [ManifestCapability.SystemControlTelephony], consent: [SensitivePermission.TelephonyModem] },

  telemetry: { manifest: [ManifestCapability.SystemControlPower], consent: [] },
  "telemetry.battery": { manifest: [ManifestCapability.SystemControlPower], consent: [] },
};

/**
 * Resolves a web permission identifier into both kernel id spaces.
 *
 * Throws for an unrecognized identifier. There is no default grant: an unknown
 * permission string must fail closed, never fall through to ambient network
 * access.
 */
export function resolveWebPermission(perm: string): WebPermissionGrant {
  const grant = WEB_PERMISSION_GRANTS[perm.toLowerCase().trim()];
  if (!grant) {
    throw new Error(
      `RAPP: unknown web permission ${JSON.stringify(perm)}; no capability is granted for an unrecognized permission`,
    );
  }
  // Copies, so a caller cannot mutate the shared table.
  return {
    capabilities: [...grant.manifest] as CapabilityType[],
    consent: [...grant.consent] as SensitivePermissionId[],
  };
}

/**
 * Maps a web permission identifier to the canonical RAPP v2 manifest capability
 * ids a package header may declare. Throws for an unrecognized identifier.
 *
 * A recognized permission the manifest ABI cannot express (geolocation,
 * notifications) yields an empty array; use mapWebPermissionToConsent for the
 * consent request that does express it.
 */
export function mapWebPermissionToCapabilities(perm: string): CapabilityType[] {
  return resolveWebPermission(perm).capabilities;
}

/**
 * Maps a web permission identifier to the consent-broker permissions the user
 * must approve at runtime. These are never written into a manifest header.
 */
export function mapWebPermissionToConsent(perm: string): SensitivePermissionId[] {
  return resolveWebPermission(perm).consent;
}

export class RappBuilder {
  private packageId = "";
  private publisherKey: Uint8Array | null = null;
  private version: [number, number, number] = [1, 0, 0];
  private arch: RappArch = RappArch.X86_64;
  private capabilities: CapabilityType[] = [];
  private limits: ResourceLimits = { maxCpuTicks: 20000n, maxMemoryBytes: 65536n };
  private payload: Uint8Array = new Uint8Array(0);

  public static create(packageId: string): RappBuilder {
    const b = new RappBuilder();
    b.packageId = packageId;
    return b;
  }

  public setPublisherKey(publicKey: Uint8Array): this {
    if (publicKey.length !== 32) throw new Error("RAPP: publisher key must be 32 bytes");
    this.publisherKey = new Uint8Array(publicKey);
    return this;
  }

  public setArch(arch: RappArch): this { this.arch = arch; return this; }

  public setVersion(major: number, minor: number, patch: number): this {
    this.version = [major, minor, patch];
    return this;
  }

  public addCapability(cap: CapabilityType): this {
    if (!this.capabilities.includes(cap)) this.capabilities.push(cap);
    return this;
  }

  public setLimits(maxCpuTicks: bigint, maxMemoryBytes: bigint): this {
    this.limits = { maxCpuTicks, maxMemoryBytes };
    return this;
  }

  public setPayload(elfPayload: Uint8Array): this {
    this.payload = new Uint8Array(elfPayload);
    return this;
  }

  /**
   * Builds the unsigned RAPP v2 binary. The kernel will reject it until
   * a valid Ed25519 signature is inserted and the publisher is trusted.
   */
  public buildBinary(): Uint8Array {
    if (!this.packageId) throw new Error("RAPP: package ID cannot be empty");
    if (!this.publisherKey) throw new Error("RAPP: publisher public key is required");
    if (this.capabilities.length > 1024) throw new Error("RAPP: too many capabilities");
    // Fail closed on identifiers the manifest ABI does not define. The kernel
    // rejects anything outside 1..=30 (parse_rapp's `!(1..=30).contains(&cap)`
    // check in kernel/src/package/rapp.rs), including the service-only types
    // 31..39 that are never grantable from a header.
    for (const cap of this.capabilities) {
      if (!isManifestCapability(cap)) {
        throw new Error(`RAPP: capability ${cap} is outside the RAPP v2 manifest ladder (1..=30)`);
      }
    }

    const enc = new TextEncoder();
    const digest = sha256(this.payload);
    const headerSize = 0xB4;
    const out = new Uint8Array(headerSize + this.capabilities.length * 2 + this.payload.length);
    const view = new DataView(out.buffer, out.byteOffset, out.byteLength);

    out.set([0x52, 0x41, 0x50, 0x50], 0x00);
    view.setUint16(0x04, 2, true);
    view.setUint16(0x06, this.arch, true);

    const pkgBytes = enc.encode(this.packageId);
    out.set(pkgBytes.subarray(0, 32), 0x08);
    out.set(this.publisherKey, 0x28);

    view.setUint16(0x48, this.version[0], true);
    view.setUint16(0x4A, this.version[1], true);
    view.setUint16(0x4C, this.version[2], true);
    view.setUint16(0x4E, this.capabilities.length, true);
    out.set(digest, 0x50);

    // Signature field 0x70..0xAF remains zero until signing.
    view.setUint32(0xB0, this.payload.length, true);

    let offset = 0xB4;
    for (const cap of this.capabilities) {
      view.setUint16(offset, cap, true);
      offset += 2;
    }
    out.set(this.payload, offset);
    return out;
  }

  public buildSigned(privateKey: Uint8Array): Uint8Array {
    return signRappV2(this.buildBinary(), privateKey);
  }
}
