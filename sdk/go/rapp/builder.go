package rapp

import (
	"bytes"
	"encoding/binary"
	"errors"
)

var (
	// RappMagic matches the kernel definition b"RAPP".
	RappMagic = []byte{'R', 'A', 'P', 'P'}
	// RappVersion matches the kernel definition 1.
	RappVersion uint16 = 1
)

// Builder provides a fluent API for assembling and signing .rapp packages.
type Builder struct {
	packageID    string
	publisherID  string
	version      Version
	arch         Arch
	capabilities []CapabilityType
	limits       ResourceLimits
	payload      []byte
}

// NewBuilder initializes a new .rapp package builder.
func NewBuilder(packageID string) *Builder {
	return &Builder{
		packageID: packageID,
		version:   Version{Major: 1, Minor: 0, Patch: 0},
		arch:      ArchX86_64,
		limits: ResourceLimits{
			MaxCpuTicks:    20000,
			MaxMemoryBytes: 65536,
		},
	}
}

func (b *Builder) SetPublisher(pubID string) *Builder {
	b.publisherID = pubID
	return b
}

func (b *Builder) SetArch(arch Arch) *Builder {
	b.arch = arch
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

// BuildBinary serializes the package into binary .rapp bytes matching the kernel ABI.
func (b *Builder) BuildBinary() ([]byte, error) {
	if b.packageID == "" {
		return nil, errors.New("rapp: package ID cannot be empty")
	}

	buf := new(bytes.Buffer)

	// 1. Magic Header (4 bytes: "RAPP")
	buf.Write(RappMagic)

	// 2. Format Version (u16 LE = 1)
	binary.Write(buf, binary.LittleEndian, RappVersion)

	// 3. Architecture (u16 LE)
	binary.Write(buf, binary.LittleEndian, uint16(b.arch))

	// 4. Package ID (32 bytes, null-padded)
	var pkgIDBuf [32]byte
	copy(pkgIDBuf[:], []byte(b.packageID))
	buf.Write(pkgIDBuf[:])

	// 5. Publisher ID (16 bytes, null-padded)
	var pubIDBuf [16]byte
	copy(pubIDBuf[:], []byte(b.publisherID))
	buf.Write(pubIDBuf[:])

	// 6. Version Tuple (3 * u16 LE)
	binary.Write(buf, binary.LittleEndian, b.version.Major)
	binary.Write(buf, binary.LittleEndian, b.version.Minor)
	binary.Write(buf, binary.LittleEndian, b.version.Patch)

	// 7. Capability count (u16 LE)
	binary.Write(buf, binary.LittleEndian, uint16(len(b.capabilities)))

	// 8. SHA-256 Digest (32 bytes)
	digest := Sha256Digest(b.payload)
	buf.Write(digest[:])

	// 9. Ed25519 signature slot (64 bytes zeros)
	var sigBuf [64]byte
	buf.Write(sigBuf[:])

	// 10. Payload length (u32 LE)
	binary.Write(buf, binary.LittleEndian, uint32(len(b.payload)))

	// 11. Capabilities (N * u16 LE)
	for _, cap := range b.capabilities {
		binary.Write(buf, binary.LittleEndian, uint16(cap))
	}

	// 12. Payload bytes
	buf.Write(b.payload)

	return buf.Bytes(), nil
}
