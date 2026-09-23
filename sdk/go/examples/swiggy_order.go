package main

import (
	"encoding/hex"
	"fmt"
	"log"

	"github.com/interos-platform/amcp-go-sdk/amcp"
)

func main() {
	sovereignDevKey := []byte("INTER_OS_SOVEREIGN_DEV_KEY_2026!")

	wasmPayload := []byte("\x00asm\x01\x00\x00\x00SWIGGY_FOOD_ORDER_INTENT_WASM_V1")

	builder := amcp.NewBuilder("swiggy.food_order").
		SetPublisher("did:inter:publisher:swiggy_verified").
		SetVersion(1, 0, 4).
		AddCapability(amcp.CapSerialOutput).
		AddCapability(amcp.CapIpcSend).
		AddCapability(amcp.CapNetworkSend).
		SetLimits(20000, 65536).
		SetPayload(wasmPayload)

	sig := builder.Sign(sovereignDevKey)
	bin, err := builder.BuildBinary()
	if err != nil {
		log.Fatalf("Failed to build AMCP binary: %v", err)
	}

	fmt.Printf("Successfully compiled sovereign AMCP package: 'swiggy.food_order'\n")
	fmt.Printf("Binary Size: %d bytes\n", len(bin))
	fmt.Printf("Developer Signature: %s\n", hex.EncodeToString(sig[:]))
}
