package rapp

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/binary"
)

// Sha256Digest computes standard SHA-256 digest of byte buffer.
func Sha256Digest(data []byte) [32]byte {
	return sha256.Sum256(data)
}

// ComputeSignature calculates the HMAC-SHA256 developer attestation tag over package metadata.
func ComputeSignature(packageID string, ver Version, payloadDigest [32]byte, developerKey []byte) [32]byte {
	var msg []byte
	msg = append(msg, []byte(packageID)...)

	var vBuf [6]byte
	binary.LittleEndian.PutUint16(vBuf[0:2], ver.Major)
	binary.LittleEndian.PutUint16(vBuf[2:4], ver.Minor)
	binary.LittleEndian.PutUint16(vBuf[4:6], ver.Patch)
	msg = append(msg, vBuf[:]...)

	msg = append(msg, payloadDigest[:]...)

	mac := hmac.New(sha256.New, developerKey)
	mac.Write(msg)
	var out [32]byte
	copy(out[:], mac.Sum(nil))
	return out
}
