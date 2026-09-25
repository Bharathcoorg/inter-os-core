package rapp

import (
	"crypto/ed25519"
	"crypto/sha256"
	"errors"
)

const (
	rappSignatureOffset = 0x70
	rappSignatureSize   = 64
	rappHeaderSize      = 0xB4
)

// Sha256Digest computes standard SHA-256 digest.
func Sha256Digest(data []byte) [32]byte {
	return sha256.Sum256(data)
}

// SignBinary signs the canonical RAPP v2 message: the complete package with
// the 64-byte signature field at 0x70 replaced by zero bytes.
func SignBinary(unsigned []byte, privateKey []byte) ([]byte, error) {
	if len(unsigned) < rappHeaderSize {
		return nil, errors.New("rapp: package shorter than v2 header")
	}
	if len(privateKey) != ed25519.PrivateKeySize && len(privateKey) != ed25519.SeedSize {
		return nil, errors.New("rapp: Ed25519 private key must be 32-byte seed or 64-byte private key")
	}
	key := ed25519.PrivateKey(privateKey)
	if len(privateKey) == ed25519.SeedSize {
		key = ed25519.NewKeyFromSeed(privateKey)
	}
	message := append([]byte(nil), unsigned...)
	for i := 0; i < rappSignatureSize; i++ {
		message[rappSignatureOffset+i] = 0
	}
	sig := ed25519.Sign(key, message)
	out := append([]byte(nil), unsigned...)
	copy(out[rappSignatureOffset:rappSignatureOffset+rappSignatureSize], sig)
	return out, nil
}
