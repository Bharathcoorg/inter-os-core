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
 * Maps standard web permission identifiers to canonical RAPP CapabilityType constants.
 */
export function mapWebPermissionToCapabilities(perm: string): CapabilityType[] {
  switch (perm.toLowerCase().trim()) {
    case "camera":
    case "cameracapture":
      return [4 as CapabilityType]; // CameraCapture
    case "microphone":
    case "mic":
      return [7 as CapabilityType]; // Microphone
    case "geolocation":
    case "location":
      return [8 as CapabilityType]; // Location
    case "storage":
    case "persistent-storage":
    case "files":
      return [CapabilityType.FileSystemRead, CapabilityType.FileSystemWrite];
    case "filesystemread":
    case "vfs.read":
      return [CapabilityType.FileSystemRead];
    case "filesystemwrite":
    case "vfs.write":
      return [CapabilityType.FileSystemWrite];
    case "notifications":
    case "notification":
      return [14 as CapabilityType]; // Notifications
    case "sensor":
    case "sensors":
    case "accelerometer":
    case "gyroscope":
      return [12 as CapabilityType]; // Sensors
    case "network":
    default:
      return [CapabilityType.NetworkSocket, CapabilityType.NetworkSend, CapabilityType.NetworkReceive];
  }
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
