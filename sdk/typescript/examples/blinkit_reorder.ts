/**
 * Example: Assembling and signing a verified Blinkit 10-Minute Groceries AMCP Package.
 */
import { AmcpBuilder, CapabilityType } from "../src/index.ts";

// Official sovereign signing key
const SOVEREIGN_DEV_KEY = new TextEncoder().encode("INTER_OS_SOVEREIGN_DEV_KEY_2026!");

// Minimal valid WASM binary payload
const wasmPayload = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
  0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
  0x03, 0x02, 0x01, 0x00,
  0x0a, 0x04, 0x01, 0x02, 0x00, 0x0b,
]);

const builder = AmcpBuilder.create("blinkit.cart_checkout")
  .setPublisher("did:inter:publisher:blinkit_official")
  .setVersion(1, 2, 0)
  .addCapability(CapabilityType.SerialOutput)
  .addCapability(CapabilityType.IpcSend)
  .setLimits(15000n, 65536n)
  .setPayload(wasmPayload);

const signature = builder.sign(SOVEREIGN_DEV_KEY);
const amcpBinary = builder.buildBinary();

console.log("Assembled AMCP Binary size:", amcpBinary.length, "bytes");
console.log("Developer Signature (hex):", Buffer.from(signature).toString("hex"));
