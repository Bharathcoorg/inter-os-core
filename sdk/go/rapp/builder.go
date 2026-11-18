// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

package rapp

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"strings"
)

var RappMagic = []byte{'R', 'A', 'P', 'P'}
const RappVersion uint16 = 2

// ErrUnknownWebPermission is returned for a web permission string that has no
// canonical resolution. It is the fail-closed answer: an unrecognized name must
// never be translated into a default grant.
var ErrUnknownWebPermission = errors.New("rapp: unknown web permission")

// webPermissionGrant records one web permission in BOTH kernel ID spaces.
//
// manifest holds RAPP v2 header identifiers (CapabilityType). consent holds
// consent-broker identifiers (SensitivePermission) that the kernel mints at
// runtime after the user approves. A permission may legitimately have an empty
// manifest list: geolocation and notifications have no RAPP v2 identifier at all
// (kernel/src/package/dependency.rs, from_rapp_capabilities, derives neither from
// any manifest ID), so the only honest grant for them is a consent request.
type webPermissionGrant struct {
	manifest []CapabilityType
	consent  []SensitivePermission
}

// webPermissionGrants is the single authoritative web-permission table.
//
// Every manifest identifier below is taken from the kernel's own minter,
// kernel/src/task/user.rs (capabilities_from_rapp_with_trust), and cross-checked
// against the consent policy in kernel/src/package/dependency.rs
// (from_rapp_capabilities) and docs/RAPP_FORMAT.md section 3:
//
//	camera      -> 22 SystemControl @ SERVICE_CAMERA_CONTROL_OBJECT
//	microphone  -> 21 SystemControl @ SERVICE_AUDIO_CONTROL_OBJECT
//	sensor      -> 23 SystemControl @ SERVICE_SENSOR_CONTROL_OBJECT
//	telephony   -> 20 SystemControl @ SERVICE_TELEPHONY_CONTROL_OBJECT
//	telemetry   -> 19 SystemControl @ SERVICE_POWER_CONTROL_OBJECT
//	audio       -> 28 AudioStream, AC'97 PCM output
//
// WHY camera is not 4, microphone not 7 and geolocation not 8: those numbers are
// the CONSENT space (kernel/src/capability/permission.rs:45,51,53). In the kernel
// raw type space the same numbers are HardwarePortIo, ProcessSpawn and
// ProcessKill (kernel/src/capability/mod.rs, CapabilityType::to_raw). The minter
// used to pass them straight through, so a camera/mic/location prompt handed a
// web app unrestricted x86 port I/O and process spawn/kill authority. It now
// refuses 4/7/8 without system platform trust and reshapes them through
// sensitive_manifest_shape, but a web permission still has no business naming
// them: the scoped service grants above are the correct declaration.
//
// This table is mirrored by sdk/typescript/src/builder.ts and
// tools/port_web.py; tests/test_permission_mapping_contract.py pins all three
// against each other and against the kernel.
var webPermissionGrants = map[string]webPermissionGrant{
	"camera":        {manifest: []CapabilityType{CapSystemControlCamera}, consent: []SensitivePermission{PermCameraCapture}},
	"cameracapture": {manifest: []CapabilityType{CapSystemControlCamera}, consent: []SensitivePermission{PermCameraCapture}},

	"microphone": {manifest: []CapabilityType{CapSystemControlAudio}, consent: []SensitivePermission{PermMicrophone}},
	"mic":        {manifest: []CapabilityType{CapSystemControlAudio}, consent: []SensitivePermission{PermMicrophone}},

	"sensor":        {manifest: []CapabilityType{CapSystemControlSensor}, consent: []SensitivePermission{PermSensors}},
	"sensors":       {manifest: []CapabilityType{CapSystemControlSensor}, consent: []SensitivePermission{PermSensors}},
	"accelerometer": {manifest: []CapabilityType{CapSystemControlSensor}, consent: []SensitivePermission{PermSensors}},
	"gyroscope":     {manifest: []CapabilityType{CapSystemControlSensor}, consent: []SensitivePermission{PermSensors}},

	// Consent-only: the RAPP v2 ladder defines no GNSS object. Location is
	// minted by the broker as HardwarePortIo @ 0xCA02 (permission.rs,
	// SensitivePermission::to_capability, Location arm).
	"geolocation": {consent: []SensitivePermission{PermLocation}},
	"location":    {consent: []SensitivePermission{PermLocation}},

	// Consent-only: NotificationPost (syscall 110) authorizes on the caller's
	// own package identity, not on a manifest capability. EventPoll (17) is
	// input/event-queue authority and NetworkReceive (14) is packet receive;
	// neither is a notification grant.
	"notifications": {consent: []SensitivePermission{PermNotifications}},
	"notification":  {consent: []SensitivePermission{PermNotifications}},

	"storage":            {manifest: []CapabilityType{CapFileSystemRead, CapFileSystemWrite}, consent: []SensitivePermission{PermFiles}},
	"persistent-storage": {manifest: []CapabilityType{CapFileSystemRead, CapFileSystemWrite}, consent: []SensitivePermission{PermFiles}},
	"files":              {manifest: []CapabilityType{CapFileSystemRead, CapFileSystemWrite}, consent: []SensitivePermission{PermFiles}},
	"filesystemread":     {manifest: []CapabilityType{CapFileSystemRead}},
	"vfs.read":           {manifest: []CapabilityType{CapFileSystemRead}},
	"filesystemwrite":    {manifest: []CapabilityType{CapFileSystemWrite}},
	"vfs.write":          {manifest: []CapabilityType{CapFileSystemWrite}},

	// Network is granted only when it is asked for by name. It is never a
	// fallback for an unrecognized permission.
	"network":        {manifest: []CapabilityType{CapNetworkSocket, CapNetworkSend, CapNetworkReceive}, consent: []SensitivePermission{PermNetworkSend}},
	"networksocket":  {manifest: []CapabilityType{CapNetworkSocket}},
	"networksend":    {manifest: []CapabilityType{CapNetworkSend}},
	"networkreceive": {manifest: []CapabilityType{CapNetworkReceive}},

	"audio":          {manifest: []CapabilityType{CapAudioStream}, consent: []SensitivePermission{PermAudioSynthesis}},
	"audio.feedback": {manifest: []CapabilityType{CapAudioStream}, consent: []SensitivePermission{PermAudioSynthesis}},
	"audiosynthesis": {manifest: []CapabilityType{CapAudioStream}, consent: []SensitivePermission{PermAudioSynthesis}},

	"telephony":      {manifest: []CapabilityType{CapSystemControlTelephony}, consent: []SensitivePermission{PermTelephonyModem}},
	"telephonymodem": {manifest: []CapabilityType{CapSystemControlTelephony}, consent: []SensitivePermission{PermTelephonyModem}},
	"phone":          {manifest: []CapabilityType{CapSystemControlTelephony}, consent: []SensitivePermission{PermTelephonyModem}},
	"sms":            {manifest: []CapabilityType{CapSystemControlTelephony}, consent: []SensitivePermission{PermTelephonyModem}},

	"telemetry":         {manifest: []CapabilityType{CapSystemControlPower}},
	"telemetry.battery": {manifest: []CapabilityType{CapSystemControlPower}},
}

func resolveWebPermission(perm string) (webPermissionGrant, error) {
	grant, ok := webPermissionGrants[strings.ToLower(strings.TrimSpace(perm))]
	if !ok {
		return webPermissionGrant{}, fmt.Errorf("%w: %q", ErrUnknownWebPermission, perm)
	}
	return grant, nil
}

// MapWebPermission maps a standard web permission identifier to the canonical
// RAPP v2 manifest capability IDs a package header may declare.
//
// An unrecognized identifier returns ErrUnknownWebPermission and no capabilities.
// A recognized identifier that the manifest ABI cannot express (geolocation,
// notifications) returns an empty slice and a nil error; use
// MapWebPermissionConsent to obtain the consent request that does express it.
//
// The returned slice is a copy: callers cannot mutate the shared table.
func MapWebPermission(perm string) ([]CapabilityType, error) {
	grant, err := resolveWebPermission(perm)
	if err != nil {
		return nil, err
	}
	return append([]CapabilityType(nil), grant.manifest...), nil
}

// MapWebPermissionConsent maps a web permission identifier to the consent-broker
// permissions (SensitivePermission) the user must approve at runtime. These are
// never written into a manifest header.
func MapWebPermissionConsent(perm string) ([]SensitivePermission, error) {
	grant, err := resolveWebPermission(perm)
	if err != nil {
		return nil, err
	}
	return append([]SensitivePermission(nil), grant.consent...), nil
}

// Builder assembles canonical RAPP v2 packages.
type Builder struct {
	packageID    string
	publisherKey [32]byte
	hasPublisher bool
	version      Version
	arch         Arch
	capabilities []CapabilityType
	limits       ResourceLimits
	payload      []byte
}

func NewBuilder(packageID string) *Builder {
	return &Builder{
		packageID: packageID,
		version: Version{Major: 1, Minor: 0, Patch: 0},
		arch: ArchX86_64,
		limits: ResourceLimits{MaxCpuTicks: 20000, MaxMemoryBytes: 65536},
	}
}

// SetPublisherKey configures the 32-byte Ed25519 public key embedded in RAPP v2.
func (b *Builder) SetPublisherKey(key []byte) *Builder {
	if len(key) == 32 {
		copy(b.publisherKey[:], key)
		b.hasPublisher = true
	}
	return b
}

// SetPublisher accepts a 64-character hexadecimal Ed25519 public key for compatibility.
func (b *Builder) SetPublisher(hexKey string) *Builder {
	decoded, err := hex.DecodeString(hexKey)
	if err == nil && len(decoded) == 32 {
		return b.SetPublisherKey(decoded)
	}
	return b
}

func (b *Builder) SetArch(arch Arch) *Builder { b.arch = arch; return b }
func (b *Builder) SetVersion(maj, min, patch uint16) *Builder {
	b.version = Version{Major: maj, Minor: min, Patch: patch}; return b
}
func (b *Builder) AddCapability(cap CapabilityType) *Builder {
	b.capabilities = append(b.capabilities, cap); return b
}
func (b *Builder) SetLimits(ticks, memBytes uint64) *Builder {
	b.limits = ResourceLimits{MaxCpuTicks: ticks, MaxMemoryBytes: memBytes}; return b
}
func (b *Builder) SetPayload(payload []byte) *Builder { b.payload = payload; return b }

// BuildBinary builds an unsigned RAPP v2 binary. The returned package is not
// executable until signed and trusted by the kernel.
func (b *Builder) BuildBinary() ([]byte, error) {
	if b.packageID == "" { return nil, errors.New("rapp: package ID cannot be empty") }
	if !b.hasPublisher { return nil, errors.New("rapp: Ed25519 publisher public key is required") }
	if len(b.capabilities) > 1024 { return nil, errors.New("rapp: too many capabilities") }
	// Fail closed on identifiers the manifest ABI does not define. The kernel
	// rejects anything outside 1..=30 (kernel/src/package/rapp.rs, parse_rapp), including
	// the service-only types 31..39 that exist in CapabilityType but are never
	// grantable from a header.
	for _, cap := range b.capabilities {
		if !IsManifestCapability(cap) {
			return nil, fmt.Errorf("rapp: capability %d is outside the RAPP v2 manifest ladder (1..=30)", uint16(cap))
		}
	}

	const headerSize = 0xB4
	capsSize := len(b.capabilities) * 2
	buf := new(bytes.Buffer)
	buf.Grow(headerSize + capsSize + len(b.payload))
	buf.Write(RappMagic)
	binary.Write(buf, binary.LittleEndian, RappVersion)
	binary.Write(buf, binary.LittleEndian, uint16(b.arch))
	var pkgID [32]byte
	copy(pkgID[:], []byte(b.packageID))
	buf.Write(pkgID[:])
	buf.Write(b.publisherKey[:])
	binary.Write(buf, binary.LittleEndian, b.version.Major)
	binary.Write(buf, binary.LittleEndian, b.version.Minor)
	binary.Write(buf, binary.LittleEndian, b.version.Patch)
	binary.Write(buf, binary.LittleEndian, uint16(len(b.capabilities)))
	digest := Sha256Digest(b.payload)
	buf.Write(digest[:])
	buf.Write(make([]byte, 64))
	binary.Write(buf, binary.LittleEndian, uint32(len(b.payload)))
	for _, cap := range b.capabilities { binary.Write(buf, binary.LittleEndian, uint16(cap)) }
	buf.Write(b.payload)
	return buf.Bytes(), nil
}

// BuildSigned builds and Ed25519-signs a complete canonical RAPP v2 package.
func (b *Builder) BuildSigned(privateKey []byte) ([]byte, error) {
	unsigned, err := b.BuildBinary()
	if err != nil { return nil, err }
	return SignBinary(unsigned, privateKey)
}
