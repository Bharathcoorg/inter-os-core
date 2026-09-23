/**
 * Cryptographic helpers for AMCP package signing and integrity attestation.
 */
import { createHash, createHmac } from "node:crypto";

export function sha256(data: Uint8Array): Uint8Array {
  return new Uint8Array(createHash("sha256").update(data).digest());
}

export function hmacSha256(key: Uint8Array, message: Uint8Array): Uint8Array {
  return new Uint8Array(createHmac("sha256", key).update(message).digest());
}

export function computeSignature(
  packageId: string,
  version: [number, number, number],
  payloadDigest: Uint8Array,
  developerKey: Uint8Array
): Uint8Array {
  const enc = new TextEncoder();
  const idBytes = enc.encode(packageId);

  // Layout: package_id || v_maj (u16 LE) || v_min (u16 LE) || v_pat (u16 LE) || payload_digest (32B)
  const totalLen = idBytes.length + 6 + payloadDigest.length;
  const buf = new Uint8Array(totalLen);

  buf.set(idBytes, 0);
  let offset = idBytes.length;

  const view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  view.setUint16(offset, version[0], true);
  view.setUint16(offset + 2, version[1], true);
  view.setUint16(offset + 4, version[2], true);
  offset += 6;

  buf.set(payloadDigest, offset);

  return hmacSha256(developerKey, buf);
}
