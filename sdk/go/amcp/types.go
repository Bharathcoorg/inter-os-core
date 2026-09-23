package amcp

// CapabilityType represents a zero-trust fine-grained hardware or system capability.
type CapabilityType uint8

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
)

// CapabilityRisk classifies the permission risk level of a capability.
type CapabilityRisk int

const (
	RiskSafe CapabilityRisk = iota
	RiskRequiresConsent
	RiskForbidden
)

// ResourceLimits defines CPU and RAM execution constraints.
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
