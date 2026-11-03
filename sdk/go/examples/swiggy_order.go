// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

package main

import (
	"encoding/hex"
	"fmt"
	"log"

	"github.com/interos-platform/rapp-go-sdk/rapp"
)

func main() {
	sovereignDevKey := []byte("INTER_OS_SOVEREIGN_DEV_KEY_2026!")

	// Minimal valid ELF payload
	elfPayload := []byte{
		0x7f, 0x45, 0x4c, 0x46, 0x02, 0x01, 0x01, 0x00,
		0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
		0x02, 0x00, 0x3e, 0x00, 0x01, 0x00, 0x00, 0x00,
		0x78, 0x00, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00,
	}

	builder := rapp.NewBuilder("swiggy.food_order").
		SetPublisher("swiggy_official").
		SetArch(rapp.ArchX86_64).
		SetVersion(1, 0, 4).
		AddCapability(rapp.CapSerialOutput).
		AddCapability(rapp.CapIpcSend).
		AddCapability(rapp.CapNetworkSend).
		SetLimits(20000, 65536).
		SetPayload(elfPayload)

	sig := builder.Sign(sovereignDevKey)
	bin, err := builder.BuildBinary()
	if err != nil {
		log.Fatalf("Failed to build .rapp binary: %v", err)
	}

	fmt.Printf("Successfully compiled sovereign .rapp package: 'swiggy.food_order'\n")
	fmt.Printf("Binary Size: %d bytes\n", len(bin))
	fmt.Printf("Developer Signature: %s\n", hex.EncodeToString(sig[:]))
}
