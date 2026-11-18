// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

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

// CapabilityType represents a kernel RAPP v2 manifest capability identifier.
//
// This is the MANIFEST ID space: the numbers a .rapp header declares and the
// kernel mints at spawn time in `capabilities_from_rapp_with_trust`
// (kernel/src/task/user.rs). It is NOT the consent-broker ID space
// (SensitivePermission below), where 4/7/8 mean CameraCapture/Microphone/
// Location. In the kernel raw type space 4 is HardwarePortIo, 7 is ProcessSpawn
// and 8 is ProcessKill, so a manifest that declares them names raw port I/O and
// process control. The minter now refuses 4/7/8 without system platform trust
// and reshapes them through the consent space; a web permission must still never
// name them. The two spaces must never be substituted for one another.
//
// The ladder below is pinned to kernel/src/capability/mod.rs
// (CapabilityType::to_raw / from_raw), kernel/src/task/user.rs
// (capabilities_from_rapp_with_trust) and docs/RAPP_FORMAT.md section 3. Package
// verification accepts 1..=30 only (kernel/src/package/rapp.rs, parse_rapp).
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

	// Domain-scoped SystemControl grants. Each is SystemControl with INVOKE
	// rights bound to one service object, so holding one confers no authority
	// over any other domain (kernel/src/task/user.rs, manifest arms 19..=23):
	//   19 SERVICE_POWER_CONTROL_OBJECT     power governor, battery telemetry
	//   20 SERVICE_TELEPHONY_CONTROL_OBJECT modem, SMS, dialer
	//   21 SERVICE_AUDIO_CONTROL_OBJECT     audio control; the microphone grant
	//   22 SERVICE_CAMERA_CONTROL_OBJECT    camera frame capture
	//   23 SERVICE_SENSOR_CONTROL_OBJECT    IMU, accelerometer, biometrics
	CapSystemControlPower     CapabilityType = 19
	CapSystemControlTelephony CapabilityType = 20
	CapSystemControlAudio     CapabilityType = 21
	CapSystemControlCamera    CapabilityType = 22
	CapSystemControlSensor    CapabilityType = 23

	// 24 rtc.read               hardware wall-clock RTC
	// 25 content.index_read     document index + intent registry
	// 26 content.intent_dispatch dispatch to a declared handler
	// 27 surface.capture        read another task's display pixels
	// 28 audio.stream           AC'97 PCM output open/write/close
	// 29 memory.heap            anonymous mapping + userspace heap
	// 30 process.threads        thread spawn + TLS
	CapRealtimeClockRead     CapabilityType = 24
	CapContentIndexRead      CapabilityType = 25
	CapContentIntentDispatch CapabilityType = 26
	CapSurfaceCapture        CapabilityType = 27
	CapAudioStream           CapabilityType = 28
	CapMemoryHeap            CapabilityType = 29
	CapProcessThreads        CapabilityType = 30
)

// ManifestCapabilityMin and ManifestCapabilityMax bound the RAPP v2 manifest
// ladder. The kernel refuses every identifier outside this range with
// "Unknown RAPP v2 capability" (kernel/src/package/rapp.rs, parse_rapp).
const (
	ManifestCapabilityMin CapabilityType = 1
	ManifestCapabilityMax CapabilityType = 30
)

// IsManifestCapability reports whether id is inside the RAPP v2 manifest ladder.
//
// WHY this exists: CapabilityType also names identifiers the kernel enum defines
// but the manifest ABI deliberately excludes (InferenceService 31 .. TlsTrustRead
// 39, kernel/src/capability/mod.rs, service-only variants). Those are minted from service policy
// for built-in clients; a manifest declaring one is rejected. Callers that build
// a manifest must check the ladder, not merely the type's existence.
func IsManifestCapability(id CapabilityType) bool {
	return id >= ManifestCapabilityMin && id <= ManifestCapabilityMax
}

// SensitivePermission is the kernel's CONSENT-BROKER permission ID space
// (kernel/src/capability/permission.rs:37-85, SensitivePermission::from_u32).
//
// These are the values a user consent prompt grants at runtime. The broker mints
// a scoped capability from them (permission.rs:189-293); they are never written
// into a .rapp manifest. CameraCapture(4), Microphone(7) and Location(8) here are
// the same numbers as HardwarePortIo(4), ProcessSpawn(7) and ProcessKill(8) in
// the manifest space, which is exactly why the two types are kept distinct.
//
// Location and Notifications have NO manifest identifier: the RAPP v2 ladder
// defines no GNSS object, and NotificationPost (syscall 110) is gated on the
// caller's own package identity rather than on a manifest capability. A package
// requests them through consent, not through the header.
//
// The capability each consent grant mints (permission.rs:189-293):
//
//	1  TelephonyModem      HardwarePortIo  READ|INVOKE        @ 0x3F8
//	2  StorageSectorCrypt  StorageRawBlock READ|WRITE|INVOKE  @ vault partition
//	3  NetworkSend         NetworkSend     WRITE|INVOKE       @ 0
//	4  CameraCapture       HardwarePortIo  READ|INVOKE        @ 0xCA00
//	5  AudioSynthesis      HardwarePortIo  WRITE|INVOKE       @ 0x61
//	6  RtcClock            TimerTickRead   READ|INVOKE        @ 0x70
//	7  Microphone          HardwarePortIo  READ|INVOKE        @ 0xCA01
//	8  Location            HardwarePortIo  READ|INVOKE        @ 0xCA02
//	9  Contacts            FileSystemRead  READ|WRITE         @ 0xC001
//	10 Files               FileSystemRead  READ|WRITE         @ 0xF11E
//	11 Sms                 HardwarePortIo  WRITE|INVOKE       @ 0x3F9
//	12 Sensors             HardwarePortIo  READ|INVOKE        @ 0x5E45
//	13 Bluetooth           HardwarePortIo  READ|WRITE|INVOKE  @ 0xB700
//	14 Notifications       IpcSend         WRITE|INVOKE       @ 0x9071
//	15 AgentTask           AgentTask       READ|EXECUTE|INVOKE @ 0x1F
//	16..20 AgentTool*      AgentTask       EXECUTE|INVOKE     @ one tool bit
type SensitivePermission uint32

const (
	PermTelephonyModem      SensitivePermission = 1
	PermStorageSectorCrypt  SensitivePermission = 2
	PermNetworkSend         SensitivePermission = 3
	PermCameraCapture       SensitivePermission = 4
	PermAudioSynthesis      SensitivePermission = 5
	PermRtcClock            SensitivePermission = 6
	PermMicrophone          SensitivePermission = 7
	PermLocation            SensitivePermission = 8
	PermContacts            SensitivePermission = 9
	PermFiles               SensitivePermission = 10
	PermSms                 SensitivePermission = 11
	PermSensors             SensitivePermission = 12
	PermBluetooth           SensitivePermission = 13
	PermNotifications       SensitivePermission = 14
	PermAgentTask           SensitivePermission = 15
	PermAgentToolCalculator SensitivePermission = 16
	PermAgentToolClock      SensitivePermission = 17
	PermAgentToolStorage    SensitivePermission = 18
	PermAgentToolSettings   SensitivePermission = 19
	PermAgentToolCapCheck   SensitivePermission = 20
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
