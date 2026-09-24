/**
 * Example: Assembling and signing a verified Blinkit 10-Minute Groceries Package (.rapp).
 */
import { RappBuilder, RappArch, CapabilityType } from "../src/index.ts";

// Official sovereign signing key
const SOVEREIGN_DEV_KEY = new TextEncoder().encode("INTER_OS_SOVEREIGN_DEV_KEY_2026!");

// Minimal valid ELF/binary payload
const elfPayload = new Uint8Array([
  0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00,
  0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
  0x02, 0x00, 0x3e, 0x00, 0x01, 0x00, 0x00, 0x00,
  0x78, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
]);

const builder = RappBuilder.create("blinkit.cart_checkout")
  .setPublisher("did:inter:pub")
  .setArch(RappArch.X86_64)
  .setVersion(1, 2, 0)
  .addCapability(CapabilityType.SerialOutput)
  .addCapability(CapabilityType.IpcSend)
  .setLimits(15000n, 65536n)
  .setPayload(elfPayload);

const signature = builder.sign(SOVEREIGN_DEV_KEY);
const rappBinary = builder.buildBinary();

console.log("Assembled .rapp Binary size:", rappBinary.length, "bytes");
console.log("Developer Signature (hex):", Buffer.from(signature).toString("hex"));
