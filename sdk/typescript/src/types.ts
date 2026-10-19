/**
 * Canonical RAPP v2 type definitions.
 */

export const RappArch = {
  X86_64: 0,
  Aarch64: 1,
  Riscv64: 2,
  Wasm: 3,
  Web: 3,
} as const;

export type RappArch = (typeof RappArch)[keyof typeof RappArch];

export const CapabilityType = {
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
} as const;

export type CapabilityType = (typeof CapabilityType)[keyof typeof CapabilityType];

export const CapabilityRisk = {
  Safe: 0,
  RequiresConsent: 1,
  Forbidden: 2,
} as const;

export type CapabilityRisk = (typeof CapabilityRisk)[keyof typeof CapabilityRisk];

export const AuditVerdict = {
  Approved: 0,
  RequiresUserConsent: 1,
  BlockedForbidden: 2,
  SignatureInvalid: 3,
} as const;

export type AuditVerdict = (typeof AuditVerdict)[keyof typeof AuditVerdict];

export interface ResourceLimits {
  maxCpuTicks: bigint;
  maxMemoryBytes: bigint;
}

export interface RappPackageMetadata {
  packageId: string;
  publisherKey: Uint8Array;
  version: [number, number, number];
  arch: RappArch;
  capabilities: CapabilityType[];
  limits?: ResourceLimits;
}
