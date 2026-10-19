package rapp

// Arch represents the CPU architecture targeted by the .rapp package.
type Arch uint16

const (
	ArchX86_64  Arch = 0
	ArchAarch64 Arch = 1
	ArchRiscv64 Arch = 2
	ArchWasm    Arch = 3
	ArchWeb     Arch = 3
)

// CapabilityType represents a kernel RAPP v2 capability identifier.
type CapabilityType uint16

const (
	CapSerialOutput     CapabilityType = 1
	CapFrameBuffer      CapabilityType = 2
	CapTimerTickRead    CapabilityType = 3
	CapHardwarePortIo   CapabilityType = 4
	CapIpcSend          CapabilityType = 5
	CapIpcReceive       CapabilityType = 6
	CapProcessSpawn     CapabilityType = 7
	CapProcessKill      CapabilityType = 8
	CapFileSystemRead   CapabilityType = 9
	CapFileSystemWrite  CapabilityType = 10
	CapStorageRawBlock  CapabilityType = 11
	CapNetworkSocket    CapabilityType = 12
	CapNetworkSend      CapabilityType = 13
	CapNetworkReceive   CapabilityType = 14
	CapShmProducer      CapabilityType = 15
	CapShmConsumer      CapabilityType = 16
	CapEventPoll        CapabilityType = 17
	CapSystemControl    CapabilityType = 18
)

// CapabilityRisk classifies the permission risk level of a capability.
type CapabilityRisk int

const (
	RiskSafe CapabilityRisk = iota
	RiskRequiresConsent
	RiskForbidden
)

// ResourceLimits defines optional tooling-side execution constraints.
type ResourceLimits struct {
	MaxCpuTicks    uint64
	MaxMemoryBytes uint64
}

// Version represents semver major.minor.patch.
type Version struct {
	Major uint16
	Minor uint16
	Patch uint16
}
