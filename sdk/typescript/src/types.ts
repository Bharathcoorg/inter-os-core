/**
 * Inter OS AMCP (Agent Model Context Protocol) Type Definitions
 */

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

export interface AmcpPackageMetadata {
  packageId: string;
  publisherDid: string;
  version: [number, number, number];
  capabilities: CapabilityType[];
  limits: ResourceLimits;
}
