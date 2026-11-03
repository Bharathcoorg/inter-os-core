// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

/**
 * Cryptographic helpers for canonical RAPP v2 packages.
 */
import { createHash, createPrivateKey, sign as cryptoSign } from "node:crypto";

export function sha256(data: Uint8Array): Uint8Array {
  return new Uint8Array(createHash("sha256").update(data).digest());
}

function ed25519PrivateKey(privateKey: Uint8Array) {
  if (privateKey.length === 32) {
    const pkcs8Prefix = Uint8Array.from([
      0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70,
      0x04, 0x22, 0x04, 0x20,
    ]);
    const der = new Uint8Array(pkcs8Prefix.length + 32);
    der.set(pkcs8Prefix, 0);
    der.set(privateKey, pkcs8Prefix.length);
    return createPrivateKey({ key: Buffer.from(der), format: "der", type: "pkcs8" });
  }
  if (privateKey.length === 48) {
    return createPrivateKey({ key: Buffer.from(privateKey), format: "der", type: "pkcs8" });
  }
  throw new Error("RAPP: Ed25519 private key must be a 32-byte seed or 48-byte PKCS#8 key");
}

export function signRappV2(unsignedPackage: Uint8Array, privateKey: Uint8Array): Uint8Array {
  if (unsignedPackage.length < 0xB4) throw new Error("RAPP: package shorter than v2 header");
  const message = new Uint8Array(unsignedPackage);
  message.fill(0, 0x70, 0xB0);
  const signature = cryptoSign(null, Buffer.from(message), ed25519PrivateKey(privateKey));
  const out = new Uint8Array(unsignedPackage);
  out.set(signature, 0x70);
  return out;
}
