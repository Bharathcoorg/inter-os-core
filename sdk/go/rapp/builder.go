package rapp

import (
	"bytes"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"strings"
)

var RappMagic = []byte{'R', 'A', 'P', 'P'}
const RappVersion uint16 = 2

// MapWebPermission maps standard web permission identifiers to canonical RAPP capabilities.
func MapWebPermission(perm string) []CapabilityType {
	switch strings.ToLower(strings.TrimSpace(perm)) {
	case "camera", "cameracapture":
		return []CapabilityType{4} // CameraCapture
	case "microphone", "mic":
		return []CapabilityType{7} // Microphone
	case "geolocation", "location":
		return []CapabilityType{8} // Location
	case "storage", "persistent-storage", "files":
		return []CapabilityType{CapFileSystemRead, CapFileSystemWrite}
	case "filesystemread", "vfs.read":
		return []CapabilityType{CapFileSystemRead}
	case "filesystemwrite", "vfs.write":
		return []CapabilityType{CapFileSystemWrite}
	case "notifications", "notification":
		return []CapabilityType{14} // Notifications
	case "sensor", "sensors", "accelerometer", "gyroscope":
		return []CapabilityType{12} // Sensors
	default:
		return []CapabilityType{CapNetworkSocket, CapNetworkSend, CapNetworkReceive}
	}
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
