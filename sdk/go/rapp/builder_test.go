// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

package rapp

import (
	"bytes"
	"encoding/binary"
	"errors"
	"testing"
)

// Regression tests for the Go RAPP builder's web-permission mapping.
//
// The bug these pin: MapWebPermission returned camera -> [4], microphone -> [7]
// and geolocation -> [8]. Those are CONSENT-broker ids. In the kernel raw type
// space the same numbers are HardwarePortIo, ProcessSpawn and ProcessKill
// (kernel/src/capability/mod.rs, CapabilityType::to_raw), and the manifest minter
// used to pass them straight through, so an ordinary camera prompt handed a web
// app raw x86 port I/O and process control. Unrecognized strings fell through to
// a default that granted CapNetworkSocket + CapNetworkSend + CapNetworkReceive:
// ambient authority for a typo.
//
// The expected values below are written as literals, independent of the
// constants under test, and are taken from the kernel (cited by symbol, since
// line numbers move as the kernel is edited):
//
//	kernel/src/task/user.rs           capabilities_from_rapp_with_trust
//	kernel/src/capability/mod.rs      CapabilityType::to_raw / from_raw
//	kernel/src/capability/permission.rs  SensitivePermission, to_capability
//	kernel/src/package/dependency.rs  PackagePolicy::from_rapp_capabilities
//	kernel/src/package/rapp.rs        parse_rapp, manifest ladder bound 1..=30
//
// tests/test_permission_mapping_contract.py additionally cross-checks this table
// against sdk/typescript/src/builder.ts, tools/port_web.py and the kernel source.

type wantGrant struct {
	perm     string
	manifest []CapabilityType
	consent  []SensitivePermission
}

// The corrected table. geolocation and notifications have NO manifest id: the
// ladder defines no GNSS object, and NotificationPost (syscall 110) authorizes on
// the caller's own package identity, so their only honest grant is consent.
var wantGrants = []wantGrant{
	{perm: "camera", manifest: []CapabilityType{22}, consent: []SensitivePermission{4}},
	{perm: "cameracapture", manifest: []CapabilityType{22}, consent: []SensitivePermission{4}},
	{perm: "microphone", manifest: []CapabilityType{21}, consent: []SensitivePermission{7}},
	{perm: "mic", manifest: []CapabilityType{21}, consent: []SensitivePermission{7}},
	{perm: "sensor", manifest: []CapabilityType{23}, consent: []SensitivePermission{12}},
	{perm: "sensors", manifest: []CapabilityType{23}, consent: []SensitivePermission{12}},
	{perm: "accelerometer", manifest: []CapabilityType{23}, consent: []SensitivePermission{12}},
	{perm: "gyroscope", manifest: []CapabilityType{23}, consent: []SensitivePermission{12}},
	{perm: "geolocation", manifest: []CapabilityType{}, consent: []SensitivePermission{8}},
	{perm: "location", manifest: []CapabilityType{}, consent: []SensitivePermission{8}},
	{perm: "notifications", manifest: []CapabilityType{}, consent: []SensitivePermission{14}},
	{perm: "notification", manifest: []CapabilityType{}, consent: []SensitivePermission{14}},
	{perm: "storage", manifest: []CapabilityType{9, 10}, consent: []SensitivePermission{10}},
	{perm: "persistent-storage", manifest: []CapabilityType{9, 10}, consent: []SensitivePermission{10}},
	{perm: "files", manifest: []CapabilityType{9, 10}, consent: []SensitivePermission{10}},
	{perm: "filesystemread", manifest: []CapabilityType{9}, consent: []SensitivePermission{}},
	{perm: "vfs.read", manifest: []CapabilityType{9}, consent: []SensitivePermission{}},
	{perm: "filesystemwrite", manifest: []CapabilityType{10}, consent: []SensitivePermission{}},
	{perm: "vfs.write", manifest: []CapabilityType{10}, consent: []SensitivePermission{}},
	{perm: "network", manifest: []CapabilityType{12, 13, 14}, consent: []SensitivePermission{3}},
	{perm: "networksocket", manifest: []CapabilityType{12}, consent: []SensitivePermission{}},
	{perm: "networksend", manifest: []CapabilityType{13}, consent: []SensitivePermission{}},
	{perm: "networkreceive", manifest: []CapabilityType{14}, consent: []SensitivePermission{}},
	{perm: "audio", manifest: []CapabilityType{28}, consent: []SensitivePermission{5}},
	{perm: "audio.feedback", manifest: []CapabilityType{28}, consent: []SensitivePermission{5}},
	{perm: "audiosynthesis", manifest: []CapabilityType{28}, consent: []SensitivePermission{5}},
	{perm: "telephony", manifest: []CapabilityType{20}, consent: []SensitivePermission{1}},
	{perm: "telephonymodem", manifest: []CapabilityType{20}, consent: []SensitivePermission{1}},
	{perm: "phone", manifest: []CapabilityType{20}, consent: []SensitivePermission{1}},
	{perm: "sms", manifest: []CapabilityType{20}, consent: []SensitivePermission{1}},
	{perm: "telemetry", manifest: []CapabilityType{19}, consent: []SensitivePermission{}},
	{perm: "telemetry.battery", manifest: []CapabilityType{19}, consent: []SensitivePermission{}},
}

func capsEqual(got, want []CapabilityType) bool {
	if len(got) != len(want) {
		return false
	}
	for i := range got {
		if got[i] != want[i] {
			return false
		}
	}
	return true
}

func permsEqual(got, want []SensitivePermission) bool {
	if len(got) != len(want) {
		return false
	}
	for i := range got {
		if got[i] != want[i] {
			return false
		}
	}
	return true
}

func TestMapWebPermissionCameraIsSystemControlCamera(t *testing.T) {
	got, err := MapWebPermission("camera")
	if err != nil {
		t.Fatalf("camera returned an error: %v", err)
	}
	if !capsEqual(got, []CapabilityType{CapSystemControlCamera}) {
		t.Fatalf("camera = %v, want [22] SystemControlCamera", got)
	}
	if capsEqual(got, []CapabilityType{CapHardwarePortIo}) {
		t.Fatal("camera granted HardwarePortIo (4): raw x86 port I/O from a camera prompt")
	}
	consent, err := MapWebPermissionConsent("camera")
	if err != nil {
		t.Fatalf("camera consent returned an error: %v", err)
	}
	if !permsEqual(consent, []SensitivePermission{PermCameraCapture}) {
		t.Fatalf("camera consent = %v, want [4] CameraCapture", consent)
	}
}

func TestMapWebPermissionMicrophoneIsSystemControlAudio(t *testing.T) {
	got, err := MapWebPermission("microphone")
	if err != nil {
		t.Fatalf("microphone returned an error: %v", err)
	}
	if !capsEqual(got, []CapabilityType{CapSystemControlAudio}) {
		t.Fatalf("microphone = %v, want [21] SystemControlAudio", got)
	}
	if capsEqual(got, []CapabilityType{CapProcessSpawn}) {
		t.Fatal("microphone granted ProcessSpawn (7)")
	}
	consent, err := MapWebPermissionConsent("microphone")
	if err != nil {
		t.Fatalf("microphone consent returned an error: %v", err)
	}
	if !permsEqual(consent, []SensitivePermission{PermMicrophone}) {
		t.Fatalf("microphone consent = %v, want [7] Microphone", consent)
	}
}

func TestMapWebPermissionSensorIsSystemControlSensor(t *testing.T) {
	for _, perm := range []string{"sensor", "sensors", "accelerometer", "gyroscope"} {
		got, err := MapWebPermission(perm)
		if err != nil {
			t.Fatalf("%s returned an error: %v", perm, err)
		}
		if !capsEqual(got, []CapabilityType{CapSystemControlSensor}) {
			t.Fatalf("%s = %v, want [23] SystemControlSensor", perm, got)
		}
		if capsEqual(got, []CapabilityType{CapNetworkSocket}) {
			t.Fatalf("%s granted NetworkSocket (12)", perm)
		}
	}
}

func TestMapWebPermissionGeolocationIsConsentOnly(t *testing.T) {
	got, err := MapWebPermission("geolocation")
	if err != nil {
		t.Fatalf("geolocation returned an error: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("geolocation = %v, want no manifest id (the ladder defines no GNSS object)", got)
	}
	if capsEqual(got, []CapabilityType{CapProcessKill}) {
		t.Fatal("geolocation granted ProcessKill (8)")
	}
	if capsEqual(got, []CapabilityType{CapRealtimeClockRead}) {
		t.Fatal("geolocation granted RealtimeClockRead (24), which is not location")
	}
	consent, err := MapWebPermissionConsent("geolocation")
	if err != nil {
		t.Fatalf("geolocation consent returned an error: %v", err)
	}
	if !permsEqual(consent, []SensitivePermission{PermLocation}) {
		t.Fatalf("geolocation consent = %v, want [8] Location (mints HardwarePortIo @ 0xCA02)", consent)
	}
}

func TestMapWebPermissionNotificationsAreConsentOnly(t *testing.T) {
	got, err := MapWebPermission("notifications")
	if err != nil {
		t.Fatalf("notifications returned an error: %v", err)
	}
	if len(got) != 0 {
		t.Fatalf("notifications = %v, want no manifest id (syscall 110 gates on package identity)", got)
	}
	consent, err := MapWebPermissionConsent("notifications")
	if err != nil {
		t.Fatalf("notifications consent returned an error: %v", err)
	}
	if !permsEqual(consent, []SensitivePermission{PermNotifications}) {
		t.Fatalf("notifications consent = %v, want [14] Notifications (mints IpcSend @ 0x9071)", consent)
	}
}

func TestWebPermissionTableMatchesTheKernel(t *testing.T) {
	for _, want := range wantGrants {
		got, err := MapWebPermission(want.perm)
		if err != nil {
			t.Errorf("%s returned an error: %v", want.perm, err)
			continue
		}
		if !capsEqual(got, want.manifest) {
			t.Errorf("%s manifest = %v, want %v", want.perm, got, want.manifest)
		}
		consent, err := MapWebPermissionConsent(want.perm)
		if err != nil {
			t.Errorf("%s consent returned an error: %v", want.perm, err)
			continue
		}
		if !permsEqual(consent, want.consent) {
			t.Errorf("%s consent = %v, want %v", want.perm, consent, want.consent)
		}
	}
}

func TestWebPermissionTableHasNoExtraOrMissingKeys(t *testing.T) {
	want := make(map[string]bool, len(wantGrants))
	for _, g := range wantGrants {
		want[g.perm] = true
	}
	for perm := range webPermissionGrants {
		if !want[perm] {
			t.Errorf("webPermissionGrants has an unexpected key %q", perm)
		}
	}
	for perm := range want {
		if _, ok := webPermissionGrants[perm]; !ok {
			t.Errorf("webPermissionGrants is missing %q", perm)
		}
	}
}

func TestNoWebPermissionGrantsRawPortIoOrProcessControl(t *testing.T) {
	forbidden := []CapabilityType{CapHardwarePortIo, CapProcessSpawn, CapProcessKill}
	for perm := range webPermissionGrants {
		got, err := MapWebPermission(perm)
		if err != nil {
			t.Errorf("%s returned an error: %v", perm, err)
			continue
		}
		for _, bad := range forbidden {
			for _, id := range got {
				if id == bad {
					t.Errorf("%s granted forbidden manifest id %d", perm, uint16(bad))
				}
			}
		}
	}
}

func TestUnknownWebPermissionFailsClosed(t *testing.T) {
	unknown := []string{
		"", "   ", "admin", "root", "shell", "debugger", "camera;rm -rf /",
		"network-all", "*", "0", "31", "999", "not-a-real-permission",
	}
	for _, perm := range unknown {
		got, err := MapWebPermission(perm)
		if !errors.Is(err, ErrUnknownWebPermission) {
			t.Errorf("MapWebPermission(%q) error = %v, want ErrUnknownWebPermission", perm, err)
		}
		if len(got) != 0 {
			t.Errorf("MapWebPermission(%q) = %v, want no capability for an unknown permission", perm, got)
		}
		for _, id := range got {
			if id == CapNetworkSocket || id == CapNetworkSend || id == CapNetworkReceive {
				t.Errorf("MapWebPermission(%q) fell open to ambient network (%d)", perm, uint16(id))
			}
		}
		consent, err := MapWebPermissionConsent(perm)
		if !errors.Is(err, ErrUnknownWebPermission) {
			t.Errorf("MapWebPermissionConsent(%q) error = %v, want ErrUnknownWebPermission", perm, err)
		}
		if len(consent) != 0 {
			t.Errorf("MapWebPermissionConsent(%q) = %v, want empty", perm, consent)
		}
	}
}

func TestPermissionLookupNormalizesCaseAndWhitespace(t *testing.T) {
	cases := []struct {
		perm string
		want []CapabilityType
	}{
		{perm: "  CAMERA  ", want: []CapabilityType{CapSystemControlCamera}},
		{perm: "MicroPhone", want: []CapabilityType{CapSystemControlAudio}},
		{perm: "SENSOR", want: []CapabilityType{CapSystemControlSensor}},
		{perm: "Vfs.Read", want: []CapabilityType{CapFileSystemRead}},
		{perm: "Telemetry.Battery", want: []CapabilityType{CapSystemControlPower}},
		{perm: "\tGeoLocation\n", want: []CapabilityType{}},
	}
	for _, c := range cases {
		got, err := MapWebPermission(c.perm)
		if err != nil {
			t.Errorf("%q returned an error: %v", c.perm, err)
			continue
		}
		if !capsEqual(got, c.want) {
			t.Errorf("%q = %v, want %v", c.perm, got, c.want)
		}
	}
}

func TestManifestAndConsentIDSpacesAreDistinct(t *testing.T) {
	// The numbers collide; the authorities do not. Substituting one space for
	// the other is the original defect.
	if uint16(CapHardwarePortIo) != uint16(PermCameraCapture) {
		t.Fatal("expected manifest 4 and consent 4 to be the same number")
	}
	if uint16(CapProcessSpawn) != uint16(PermMicrophone) {
		t.Fatal("expected manifest 7 and consent 7 to be the same number")
	}
	if uint16(CapProcessKill) != uint16(PermLocation) {
		t.Fatal("expected manifest 8 and consent 8 to be the same number")
	}
	if CapSystemControlCamera == CapabilityType(PermCameraCapture) {
		t.Fatal("the camera manifest grant must not be the consent id")
	}
	if CapSystemControlAudio == CapabilityType(PermMicrophone) {
		t.Fatal("the microphone manifest grant must not be the consent id")
	}
}

func TestManifestLadderMatchesTheKernel(t *testing.T) {
	kernel := []struct {
		name string
		id   CapabilityType
		want uint16
	}{
		{name: "SerialOutput", id: CapSerialOutput, want: 1},
		{name: "FrameBufferDirect", id: CapFrameBuffer, want: 2},
		{name: "TimerTickRead", id: CapTimerTickRead, want: 3},
		{name: "HardwarePortIo", id: CapHardwarePortIo, want: 4},
		{name: "IpcSend", id: CapIpcSend, want: 5},
		{name: "IpcReceive", id: CapIpcReceive, want: 6},
		{name: "ProcessSpawn", id: CapProcessSpawn, want: 7},
		{name: "ProcessKill", id: CapProcessKill, want: 8},
		{name: "FileSystemRead", id: CapFileSystemRead, want: 9},
		{name: "FileSystemWrite", id: CapFileSystemWrite, want: 10},
		{name: "StorageRawBlock", id: CapStorageRawBlock, want: 11},
		{name: "NetworkSocket", id: CapNetworkSocket, want: 12},
		{name: "NetworkSend", id: CapNetworkSend, want: 13},
		{name: "NetworkReceive", id: CapNetworkReceive, want: 14},
		{name: "ShmProducer", id: CapShmProducer, want: 15},
		{name: "ShmConsumer", id: CapShmConsumer, want: 16},
		{name: "EventPoll", id: CapEventPoll, want: 17},
		{name: "SystemControl", id: CapSystemControl, want: 18},
		{name: "SystemControlPower", id: CapSystemControlPower, want: 19},
		{name: "SystemControlTelephony", id: CapSystemControlTelephony, want: 20},
		{name: "SystemControlAudio", id: CapSystemControlAudio, want: 21},
		{name: "SystemControlCamera", id: CapSystemControlCamera, want: 22},
		{name: "SystemControlSensor", id: CapSystemControlSensor, want: 23},
		{name: "RealtimeClockRead", id: CapRealtimeClockRead, want: 24},
		{name: "ContentIndexRead", id: CapContentIndexRead, want: 25},
		{name: "ContentIntentDispatch", id: CapContentIntentDispatch, want: 26},
		{name: "SurfaceCapture", id: CapSurfaceCapture, want: 27},
		{name: "AudioStream", id: CapAudioStream, want: 28},
		{name: "MemoryHeap", id: CapMemoryHeap, want: 29},
		{name: "ProcessThreads", id: CapProcessThreads, want: 30},
	}
	for _, k := range kernel {
		if uint16(k.id) != k.want {
			t.Errorf("%s = %d, want kernel id %d", k.name, uint16(k.id), k.want)
		}
	}
	if uint16(ManifestCapabilityMin) != 1 || uint16(ManifestCapabilityMax) != 30 {
		t.Errorf("manifest ladder bounds = %d..%d, want 1..30 (kernel/src/package/rapp.rs, parse_rapp)",
			uint16(ManifestCapabilityMin), uint16(ManifestCapabilityMax))
	}
}

func TestIsManifestCapabilityBounds(t *testing.T) {
	for id := CapabilityType(1); id <= 30; id++ {
		if !IsManifestCapability(id) {
			t.Errorf("IsManifestCapability(%d) = false, want true", uint16(id))
		}
	}
	// 31..39 exist in the kernel enum but are service-only and never grantable
	// from a manifest header (kernel/src/capability/mod.rs, service-only variants).
	for _, id := range []CapabilityType{0, 31, 32, 33, 34, 35, 36, 37, 38, 39, 999, 65535} {
		if IsManifestCapability(id) {
			t.Errorf("IsManifestCapability(%d) = true, want false", uint16(id))
		}
	}
}

func TestConsentTableMatchesTheKernel(t *testing.T) {
	kernel := []struct {
		name string
		id   SensitivePermission
		want uint32
	}{
		{name: "TelephonyModem", id: PermTelephonyModem, want: 1},
		{name: "StorageSectorCrypt", id: PermStorageSectorCrypt, want: 2},
		{name: "NetworkSend", id: PermNetworkSend, want: 3},
		{name: "CameraCapture", id: PermCameraCapture, want: 4},
		{name: "AudioSynthesis", id: PermAudioSynthesis, want: 5},
		{name: "RtcClock", id: PermRtcClock, want: 6},
		{name: "Microphone", id: PermMicrophone, want: 7},
		{name: "Location", id: PermLocation, want: 8},
		{name: "Contacts", id: PermContacts, want: 9},
		{name: "Files", id: PermFiles, want: 10},
		{name: "Sms", id: PermSms, want: 11},
		{name: "Sensors", id: PermSensors, want: 12},
		{name: "Bluetooth", id: PermBluetooth, want: 13},
		{name: "Notifications", id: PermNotifications, want: 14},
		{name: "AgentTask", id: PermAgentTask, want: 15},
		{name: "AgentToolCalculator", id: PermAgentToolCalculator, want: 16},
		{name: "AgentToolClock", id: PermAgentToolClock, want: 17},
		{name: "AgentToolStorage", id: PermAgentToolStorage, want: 18},
		{name: "AgentToolSettings", id: PermAgentToolSettings, want: 19},
		{name: "AgentToolCapCheck", id: PermAgentToolCapCheck, want: 20},
	}
	for _, k := range kernel {
		if uint32(k.id) != k.want {
			t.Errorf("Perm%s = %d, want kernel consent id %d", k.name, uint32(k.id), k.want)
		}
	}
}

func TestEveryGrantedIDIsInsideItsKernelRange(t *testing.T) {
	for _, want := range wantGrants {
		for _, id := range want.manifest {
			if !IsManifestCapability(id) {
				t.Errorf("%s manifest id %d is outside the 1..=30 ladder", want.perm, uint16(id))
			}
		}
		for _, id := range want.consent {
			if id < 1 || id > 20 {
				t.Errorf("%s consent id %d is outside SensitivePermission 1..=20", want.perm, uint32(id))
			}
		}
	}
}

func TestMapWebPermissionReturnsCopies(t *testing.T) {
	first, err := MapWebPermission("camera")
	if err != nil {
		t.Fatalf("camera returned an error: %v", err)
	}
	first[0] = CapHardwarePortIo
	second, err := MapWebPermission("camera")
	if err != nil {
		t.Fatalf("camera returned an error: %v", err)
	}
	if !capsEqual(second, []CapabilityType{CapSystemControlCamera}) {
		t.Fatalf("mutating a returned slice corrupted the shared table: %v", second)
	}

	firstConsent, err := MapWebPermissionConsent("camera")
	if err != nil {
		t.Fatalf("camera consent returned an error: %v", err)
	}
	firstConsent[0] = PermNetworkSend
	secondConsent, err := MapWebPermissionConsent("camera")
	if err != nil {
		t.Fatalf("camera consent returned an error: %v", err)
	}
	if !permsEqual(secondConsent, []SensitivePermission{PermCameraCapture}) {
		t.Fatalf("mutating a returned consent slice corrupted the shared table: %v", secondConsent)
	}
}

func TestBuildBinaryRejectsOutOfLadderCapabilities(t *testing.T) {
	key := bytes.Repeat([]byte{7}, 32)
	for _, bad := range []CapabilityType{0, 31, 32, 39, 999} {
		b := NewBuilder("test.ladder").SetPublisherKey(key).SetArch(ArchWeb).
			SetPayload([]byte{1, 2, 3}).AddCapability(bad)
		out, err := b.BuildBinary()
		if err == nil {
			t.Errorf("BuildBinary accepted out-of-ladder capability %d", uint16(bad))
		}
		if out != nil {
			t.Errorf("BuildBinary returned bytes for rejected capability %d", uint16(bad))
		}
	}
}

func TestBuildBinaryEmitsTheCorrectedCapabilityIDs(t *testing.T) {
	key := bytes.Repeat([]byte{7}, 32)
	b := NewBuilder("test.webapp").SetPublisherKey(key).SetArch(ArchWeb).
		SetPayload([]byte{0x7f, 'E', 'L', 'F'})
	for _, perm := range []string{"camera", "microphone", "sensor"} {
		caps, err := MapWebPermission(perm)
		if err != nil {
			t.Fatalf("%s returned an error: %v", perm, err)
		}
		for _, cap := range caps {
			b.AddCapability(cap)
		}
	}
	out, err := b.BuildBinary()
	if err != nil {
		t.Fatalf("BuildBinary failed: %v", err)
	}
	if !bytes.Equal(out[:4], RappMagic) {
		t.Fatalf("magic = %q, want RAPP", out[:4])
	}
	if got := binary.LittleEndian.Uint16(out[0x06:0x08]); got != uint16(ArchWeb) {
		t.Errorf("arch = %d, want %d", got, uint16(ArchWeb))
	}
	if ArchWasm != ArchWeb {
		t.Errorf("ArchWasm = %d, ArchWeb = %d; both must stay 3", uint16(ArchWasm), uint16(ArchWeb))
	}
	count := int(binary.LittleEndian.Uint16(out[0x4E:0x50]))
	if count != 3 {
		t.Fatalf("capability count = %d, want 3", count)
	}
	got := make([]CapabilityType, 0, count)
	for i := 0; i < count; i++ {
		off := 0xB4 + i*2
		got = append(got, CapabilityType(binary.LittleEndian.Uint16(out[off:off+2])))
	}
	want := []CapabilityType{CapSystemControlAudio, CapSystemControlCamera, CapSystemControlSensor}
	if !capsEqual(got, want) {
		t.Fatalf("header capabilities = %v, want %v", got, want)
	}
	for _, bad := range []CapabilityType{CapHardwarePortIo, CapProcessSpawn, CapProcessKill} {
		for _, id := range got {
			if id == bad {
				t.Fatalf("header declares forbidden capability %d", uint16(bad))
			}
		}
	}
	// Unsigned by default: the signature field stays zero until BuildSigned.
	if !bytes.Equal(out[0x70:0xB0], make([]byte, 64)) {
		t.Fatal("BuildBinary wrote a non-zero signature field")
	}
}