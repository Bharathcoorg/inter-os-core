package amcp

import (
	"bytes"
	"encoding/binary"
	"errors"
)

var (
	// AmcpMagic matches the kernel definition b"AMCP\x01\x00".
	AmcpMagic = []byte{0x41, 0x4d, 0x43, 0x50, 0x01, 0x00}
	AmcpVersion uint16 = 1
)

// Builder provides a fluent API for assembling and signing AMCP automations.
type Builder struct {
	packageID    string
	publisherDID string
	version      Version
	capabilities []CapabilityType
	limits       ResourceLimits
	payload      []byte
}

// NewBuilder initializes a new AMCP package builder.
func NewBuilder(packageID string) *Builder {
	return &Builder{
		packageID: packageID,
		version:   Version{Major: 1, Minor: 0, Patch: 0},
		limits: ResourceLimits{
			MaxCpuTicks:    20000,
			MaxMemoryBytes: 65536,
		},
	}
}

func (b *Builder) SetPublisher(did string) *Builder {
	b.publisherDID = did
	return b
}

func (b *Builder) SetVersion(maj, min, patch uint16) *Builder {
	b.version = Version{Major: maj, Minor: min, Patch: patch}
	return b
}

func (b *Builder) AddCapability(cap CapabilityType) *Builder {
	b.capabilities = append(b.capabilities, cap)
	return b
}

func (b *Builder) SetLimits(ticks, memBytes uint64) *Builder {
	b.limits = ResourceLimits{MaxCpuTicks: ticks, MaxMemoryBytes: memBytes}
	return b
}

func (b *Builder) SetPayload(payload []byte) *Builder {
	b.payload = payload
	return b
}

// Sign computes the developer cryptographic signature.
func (b *Builder) Sign(developerKey []byte) [32]byte {
	digest := Sha256Digest(b.payload)
	return ComputeSignature(b.packageID, b.version, digest, developerKey)
}

// BuildBinary serializes the package into binary .amcp bytes matching the kernel ABI.
func (b *Builder) BuildBinary() ([]byte, error) {
	if b.packageID == "" {
		return nil, errors.New("amcp: package ID cannot be empty")
	}

	buf := new(bytes.Buffer)

	// 1. Magic Header (6 bytes)
	buf.Write(AmcpMagic)

	// 2. Format Version (u16 LE)
	binary.Write(buf, binary.LittleEndian, AmcpVersion)

	// 3. Version Tuple (3 * u16 LE)
	binary.Write(buf, binary.LittleEndian, b.version.Major)
	binary.Write(buf, binary.LittleEndian, b.version.Minor)
	binary.Write(buf, binary.LittleEndian, b.version.Patch)

	// 4. SHA-256 Digest (32 bytes)
	digest := Sha256Digest(b.payload)
	buf.Write(digest[:])

	// 5. Resource Limits (2 * u64 LE)
	binary.Write(buf, binary.LittleEndian, b.limits.MaxCpuTicks)
	binary.Write(buf, binary.LittleEndian, b.limits.MaxMemoryBytes)

	// 6. Declared capabilities count + items
	buf.WriteByte(byte(len(b.capabilities)))
	for _, cap := range b.capabilities {
		buf.WriteByte(byte(cap))
	}

	// 7. Package ID (len + bytes)
	idBytes := []byte(b.packageID)
	buf.WriteByte(byte(len(idBytes)))
	buf.Write(idBytes)

	// 8. Publisher DID (len + bytes)
	pubBytes := []byte(b.publisherDID)
	buf.WriteByte(byte(len(pubBytes)))
	buf.Write(pubBytes)

	// 9. Payload Length (u32 LE) + payload bytes
	binary.Write(buf, binary.LittleEndian, uint32(len(b.payload)))
	buf.Write(b.payload)

	return buf.Bytes(), nil
}
