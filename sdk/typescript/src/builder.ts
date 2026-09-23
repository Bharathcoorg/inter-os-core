/**
 * Fluent Builder for Sovereign AMCP Automation Packages.
 */
import { CapabilityType, type ResourceLimits } from "./types.ts";
import { sha256, computeSignature } from "./crypto.ts";

export class AmcpBuilder {
  private packageId = "";
  private publisherDid = "";
  private version: [number, number, number] = [1, 0, 0];
  private capabilities: CapabilityType[] = [];
  private limits: ResourceLimits = {
    maxCpuTicks: 20000n,
    maxMemoryBytes: 65536n,
  };
  private payload: Uint8Array = new Uint8Array(0);

  public static create(packageId: string): AmcpBuilder {
    const b = new AmcpBuilder();
    b.packageId = packageId;
    return b;
  }

  public setPublisher(did: string): this {
    this.publisherDid = did;
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

  public setPayload(wasmOrBytecode: Uint8Array): this {
    this.payload = wasmOrBytecode;
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
   * Serializes the package into a standard binary `.amcp` format matching the kernel ABI.
   */
  public buildBinary(): Uint8Array {
    const enc = new TextEncoder();
    const idBytes = enc.encode(this.packageId);
    const pubBytes = enc.encode(this.publisherDid);
    const digest = sha256(this.payload);

    // Calculate total buffer size
    const magicLen = 6;
    const versionFmtLen = 2;
    const versionTupleLen = 6;
    const digestLen = 32;
    const limitsLen = 16;
    const capsLen = 1 + this.capabilities.length;
    const idLen = 1 + idBytes.length;
    const pubLen = 1 + pubBytes.length;
    const payloadHeaderLen = 4;
    const totalLen =
      magicLen +
      versionFmtLen +
      versionTupleLen +
      digestLen +
      limitsLen +
      capsLen +
      idLen +
      pubLen +
      payloadHeaderLen +
      this.payload.length;

    const out = new Uint8Array(totalLen);
    const view = new DataView(out.buffer, out.byteOffset, out.byteLength);

    // 1. Magic Header: "AMCP\x01\x00"
    out.set([0x41, 0x4d, 0x43, 0x50, 0x01, 0x00], 0);
    let offset = 6;

    // 2. Format Version (u16 LE = 1)
    view.setUint16(offset, 1, true);
    offset += 2;

    // 3. Version Tuple (3 * u16 LE)
    view.setUint16(offset, this.version[0], true);
    view.setUint16(offset + 2, this.version[1], true);
    view.setUint16(offset + 4, this.version[2], true);
    offset += 6;

    // 4. SHA-256 Digest (32 bytes)
    out.set(digest, offset);
    offset += 32;

    // 5. Resource Limits (2 * u64 LE)
    view.setBigUint64(offset, this.limits.maxCpuTicks, true);
    view.setBigUint64(offset + 8, this.limits.maxMemoryBytes, true);
    offset += 16;

    // 6. Declared capabilities
    out[offset++] = this.capabilities.length;
    for (const cap of this.capabilities) {
      out[offset++] = cap;
    }

    // 7. Package ID (len + string)
    out[offset++] = idBytes.length;
    out.set(idBytes, offset);
    offset += idBytes.length;

    // 8. Publisher DID (len + string)
    out[offset++] = pubBytes.length;
    out.set(pubBytes, offset);
    offset += pubBytes.length;

    // 9. Payload Length (u32 LE) + payload bytes
    view.setUint32(offset, this.payload.length, true);
    offset += 4;

    out.set(this.payload, offset);

    return out;
  }
}
