/**
 * Fluent Builder for Sovereign Rust App Packages (.rapp).
 */
import { CapabilityType, RappArch, type ResourceLimits } from "./types.ts";
import { sha256, computeSignature } from "./crypto.ts";

export class RappBuilder {
  private packageId = "";
  private publisherId = "";
  private version: [number, number, number] = [1, 0, 0];
  private arch: RappArch = RappArch.X86_64;
  private capabilities: CapabilityType[] = [];
  private limits: ResourceLimits = {
    maxCpuTicks: 20000n,
    maxMemoryBytes: 65536n,
  };
  private payload: Uint8Array = new Uint8Array(0);

  public static create(packageId: string): RappBuilder {
    const b = new RappBuilder();
    b.packageId = packageId;
    return b;
  }

  public setPublisher(pubId: string): this {
    this.publisherId = pubId;
    return this;
  }

  public setArch(arch: RappArch): this {
    this.arch = arch;
    return this;
  }

  public setVersion(major: number, minor: number, patch: number): this {
    this.version = [major, minor, patch];
    return this;
  }

  public addCapability(cap: CapabilityType): this {
    if (!this.capabilities.includes(cap)) {
      this.capabilities.push(cap);
    }
    return this;
  }

  public setLimits(maxCpuTicks: bigint, maxMemoryBytes: bigint): this {
    this.limits = { maxCpuTicks, maxMemoryBytes };
    return this;
  }

  public setPayload(elfPayload: Uint8Array): this {
    this.payload = elfPayload;
    return this;
  }

  /**
   * Computes the HMAC-SHA256 developer attestation signature for this package.
   */
  public sign(developerKey: Uint8Array): Uint8Array {
    const digest = sha256(this.payload);
    return computeSignature(this.packageId, this.version, digest, developerKey);
  }

  /**
   * Serializes the package into a standard binary `.rapp` format matching the kernel ABI:
   * 0x00: "RAPP" (4B)
   * 0x04: Version format (2B LE = 1)
   * 0x06: Target arch (2B LE: 0=x86_64, 1=aarch64, 2=riscv64)
   * 0x08: Package ID (32B null-padded)
   * 0x28: Publisher ID (16B null-padded)
   * 0x38: Version major (2B LE)
   * 0x3A: Version minor (2B LE)
   * 0x3C: Version patch (2B LE)
   * 0x3E: Capability count (2B LE)
   * 0x40: SHA-256 digest (32B)
   * 0x60: Ed25519 signature slot (64B)
   * 0xA0: Payload length (4B LE)
   * 0xA4: Capabilities (cap_count * 2B LE)
   * 0xA4 + caps: Payload bytes
   */
  public buildBinary(): Uint8Array {
    const enc = new TextEncoder();
    const digest = sha256(this.payload);

    const headerSize = 0xA4;
    const capsSize = this.capabilities.length * 2;
    const totalLen = headerSize + capsSize + this.payload.length;

    const out = new Uint8Array(totalLen);
    const view = new DataView(out.buffer, out.byteOffset, out.byteLength);

    // 1. Magic "RAPP"
    out.set([0x52, 0x41, 0x50, 0x50], 0x00);

    // 2. Format Version (u16 LE = 1)
    view.setUint16(0x04, 1, true);

    // 3. Target Architecture (u16 LE)
    view.setUint16(0x06, this.arch, true);

    // 4. Package ID (32 bytes null-padded)
    const pkgBytes = enc.encode(this.packageId);
    out.set(pkgBytes.subarray(0, 32), 0x08);

    // 5. Publisher ID (16 bytes null-padded)
    const pubBytes = enc.encode(this.publisherId);
    out.set(pubBytes.subarray(0, 16), 0x28);

    // 6. Version (3 * u16 LE)
    view.setUint16(0x38, this.version[0], true);
    view.setUint16(0x3A, this.version[1], true);
    view.setUint16(0x3C, this.version[2], true);

    // 7. Capability count (u16 LE)
    view.setUint16(0x3E, this.capabilities.length, true);

    // 8. SHA-256 Digest (32 bytes)
    out.set(digest, 0x40);

    // 9. Signature (64 bytes zeros)
    // already 0 initialized

    // 10. Payload length (u32 LE)
    view.setUint32(0xA0, this.payload.length, true);

    // 11. Capabilities (u16 LE each)
    let offset = 0xA4;
    for (const cap of this.capabilities) {
      view.setUint16(offset, cap, true);
      offset += 2;
    }

    // 12. Payload bytes
    out.set(this.payload, offset);

    return out;
  }
}
