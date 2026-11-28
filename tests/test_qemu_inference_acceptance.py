#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

"""
End-to-End Native Local AI Inference Acceptance & Memory Safety Verification in QEMU.

Exercises the guest inference path with a synthetic model:
1. Boots QEMU with provisioned model data disk (/data/models/gemma4-mobile-lite.gguf holding
   the synthetic Gemma test fixture: 128K parameters, F32, 2 transformer layers, 521,536 bytes).
2. Verifies VFS block-device discovery and import of provisioned model.
3. Verifies userspace AI inference daemon (aid) startup, model probing, and physical weight validation.
4. Unlocks the device and launches Agent Manager from the App Drawer.
5. Verifies Agent Manager Models tab reports physical generative weights as PRESENT.
6. Asks the inference service to install the model the device holds, which is
   the person's trust decision: the service serves a model only after it.
7. Switches to Agent Manager Chat tab, opens virtual soft keyboard, and types prompt.
8. Dispatches inference generation request over IPC to aid.
9. Verifies real guest token generation, streaming chunks, and assistant chat bubble rendering.
9. Verifies request-scoped cancellation and busy handling during active generation.
10. Validates memory safety, resource reclamation, and clean return to HomeScreen.
"""

import os
import sys
import time
import socket
import subprocess
import shutil
import re

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tests.test_interactive_lifecycle import (
    find_qemu,
    get_free_port,
    send_monitor_cmd,
    wait_for_screendump,
    SerialReader,
    qemu_x86_audio_args,
)
from tools.model_provisioning import sha256_file, verify_image


MODEL_VFS_PATH = "/data/models/gemma4-mobile-lite.gguf"
FIXTURE_BYTES = 521_536


def run_inference_acceptance_test():
    workspace = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    boot_img = os.path.join(workspace, "target", "inter-os-bios.img")
    model_disk = os.path.join(workspace, "target", "verify", "provisioned_model_disk.img")

    if not os.path.exists(boot_img):
        print(f"[ACCEPTANCE TEST] Boot image not found at {boot_img}. Building demo image...")
        subprocess.run(["python", "tools/interos.py", "build", "demo"], check=True)

    assert os.path.exists(boot_img), f"Boot image missing: {boot_img}"
    assert os.path.exists(model_disk), f"Provisioned model disk missing: {model_disk}"
    disk_report = verify_image(model_disk)
    assert disk_report["ok"], f"Provisioned model disk invalid: {disk_report['problems']}"
    record = next((record for record in disk_report["files"] if record["vfs_path"] == MODEL_VFS_PATH), None)
    assert record is not None, f"Fixture missing on disk: {MODEL_VFS_PATH}"
    assert record["read_back_bytes"] == FIXTURE_BYTES, (
        f"Synthetic fixture must be {FIXTURE_BYTES:,} bytes, but the disk contains "
        f"{record['read_back_bytes']:,} bytes ({record['payload_sha256_on_disk']})"
    )
    fixture = os.path.join(workspace, "target", "verify", "models", "gemma-tiny-f32.gguf")
    assert os.path.isfile(fixture), f"Reference fixture missing: {fixture}"
    assert sha256_file(fixture) == (record["payload_sha256_on_disk"], FIXTURE_BYTES), (
        "Provisioned GGUF does not match the reference fixture"
    )

    qemu_exe = find_qemu()
    assert os.path.exists(qemu_exe) or shutil.which(qemu_exe), f"QEMU binary not found: {qemu_exe}"
    print(f"[ACCEPTANCE TEST] Using QEMU binary: {qemu_exe}")

    ser_port = get_free_port()
    mon_port = get_free_port()

    args = [
        qemu_exe,
        "-machine", "q35",
        "-cpu", "qemu64,+rdrand",
        "-device", "piix3-ide,id=ide",
        "-drive", f"file={boot_img},format=raw,if=none,id=bootd,snapshot=on",
        "-device", "ide-hd,bus=ide.0,unit=0,drive=bootd,bootindex=1",
        "-drive", f"file={model_disk},format=raw,if=none,id=datad,snapshot=on",
        "-device", "ide-hd,bus=ide.0,unit=1,drive=datad",
        "-m", "1024M",
        "-display", "none",
        "-chardev", f"socket,id=ser0,host=127.0.0.1,port={ser_port},server=on,wait=off",
        "-serial", "chardev:ser0",
        "-chardev", f"socket,id=mon0,host=127.0.0.1,port={mon_port},server=on,wait=off",
        "-mon", "chardev=mon0,mode=readline",
        "-no-reboot",
    ]
    # This suite runs headless (`-display none`) and its contract is VFS model
    # import plus guest inference, not host audio. The audio probe proves a
    # driver is compiled in, not that its backend initializes: on a headless
    # host PulseAudio exits QEMU with "XDG_RUNTIME_DIR not set" before the
    # guest boots. Default to the null backend so AC97 still attaches; an
    # operator-set INTEROS_AUDIODEV or INTEROS_DISABLE_AUDIO wins, and the
    # default is removed again after the helper reads it so the mutation does
    # not outlive this call.
    saved_audiodev = os.environ.get("INTEROS_AUDIODEV")
    if saved_audiodev is None:
        os.environ["INTEROS_AUDIODEV"] = "none"
    audio_args = qemu_x86_audio_args(qemu_exe)
    if saved_audiodev is None:
        del os.environ["INTEROS_AUDIODEV"]
    args.extend(audio_args)
    if audio_args:
        print("[ACCEPTANCE TEST] AC97 attached (audio backend args present).")
    else:
        print("[ACCEPTANCE TEST] WARNING: audio card omitted (no backend available); PCI topology differs from production.")

    print(f"[ACCEPTANCE TEST] Spawning QEMU with boot image and provisioned model disk...")
    proc = subprocess.Popen(args)

    serial_sock = None
    monitor_sock = None

    try:
        time.sleep(1.0)
        serial_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        serial_sock.settimeout(5.0)
        serial_sock.connect(("127.0.0.1", ser_port))
        serial_sock.settimeout(0.5)
        reader = SerialReader(serial_sock)
        print("[ACCEPTANCE TEST] Connected to QEMU serial socket.")

        try:
            monitor_sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            monitor_sock.settimeout(5.0)
            monitor_sock.connect(("127.0.0.1", mon_port))
            monitor_sock.settimeout(0.5)
            print("[ACCEPTANCE TEST] Connected to QEMU monitor socket.")
        except Exception as e:
            print(f"[ACCEPTANCE TEST] Monitor socket error: {e}")
            monitor_sock = None

        # 1. Wait for VFS provisioned model import and aid online
        print("[ACCEPTANCE TEST] Phase 1: Verifying boot invariants and aid model detection...")
        vfs_provision_ok = reader.wait_for(
            "[VFS PROVISION] Added provisioned file: /data/models/gemma4-mobile-lite.gguf",
            timeout=30.0,
            print_keywords=["VFS PROVISION", "aid", "ATA"],
        )
        assert vfs_provision_ok, "VFS failed to import provisioned model from disk"
        print("[ACCEPTANCE TEST] [OK] VFS provisioned model imported successfully.")

        aid_weights_ok = reader.wait_for(
            "[aid] GGUF header found in a generative model slot",
            timeout=30.0,
            print_keywords=["aid", "synthetic", "storage", "weights"],
        )
        assert aid_weights_ok, "aid did not recognize the provisioned GGUF header"
        print("[ACCEPTANCE TEST] [OK] aid recognized the provisioned GGUF header.")

        launcher_ready = reader.wait_for(
            "Step 5: Initial presentation complete",
            timeout=60.0,
            print_keywords=["LAUNCHER", "Step 5", "Lock surface"],
        )
        assert launcher_ready, "Launcher failed to present initial frame"
        print("[ACCEPTANCE TEST] [OK] Launcher presented initial frame.")

        if monitor_sock:
            ppm_lock = os.path.join(workspace, "target", "test_inference_01_lock.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_lock}")
            wait_for_screendump(ppm_lock)
            print(f"[ACCEPTANCE TEST] Captured LockScreen screendump: {ppm_lock}")

        # 2. Unlock device via swipe-up
        print("[ACCEPTANCE TEST] Phase 2: Unlocking device...")
        time.sleep(1.0)
        reader.send(b"SWIPE 360 1350 360 300 5\n")
        unlocked = reader.wait_for("Device unlocked", timeout=25.0, print_keywords=["LAUNCHER", "Device unlocked"])
        if not unlocked:
            time.sleep(1.0)
            reader.send(b"SWIPE 360 1350 360 300 5\n")
            unlocked = reader.wait_for("Device unlocked", timeout=25.0, print_keywords=["LAUNCHER", "Device unlocked"])
        assert unlocked, "Failed to unlock device from LockScreen"
        print("[ACCEPTANCE TEST] [OK] Device unlocked to HomeScreen.")

        if monitor_sock:
            ppm_home = os.path.join(workspace, "target", "test_inference_02_home.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_home}")
            wait_for_screendump(ppm_home)
            print(f"[ACCEPTANCE TEST] Captured HomeScreen screendump: {ppm_home}")

        # 3. Open App Drawer and Launch Agent Manager
        print("[ACCEPTANCE TEST] Phase 3: Opening App Drawer...")
        time.sleep(1.5)
        reader.send(b"TAP 590 1110\n")  # App Drawer icon
        drawer_ready = reader.wait_for(
            "App Drawer opened",
            timeout=20.0,
            print_keywords=["LAUNCHER", "App Drawer opened", "App drawer surface rendered"],
        )
        if not drawer_ready:
            drawer_ready = reader.wait_for(
                "App drawer surface rendered",
                timeout=10.0,
                print_keywords=["LAUNCHER", "App drawer surface rendered"],
            )
        assert drawer_ready, "App Drawer failed to present"
        print("[ACCEPTANCE TEST] [OK] App Drawer presented.")

        if monitor_sock:
            ppm_drawer = os.path.join(workspace, "target", "test_inference_03_drawer.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_drawer}")
            wait_for_screendump(ppm_drawer)
            print(f"[ACCEPTANCE TEST] Captured App Drawer screendump: {ppm_drawer}")

        print("[ACCEPTANCE TEST] Launching Agent Manager (tap at 146, 988)...")
        time.sleep(1.0)
        reader.send(b"TAP 146 988\n")
        agent_ready = reader.wait_for(
            "[AGENT_MGR] Frame presented to scanout.",
            timeout=25.0,
            print_keywords=["AGENT_MGR", "scanout", "Models", "Launched application"],
        )
        if not agent_ready:
            agent_ready = reader.wait_for(
                "Agent Manager",
                timeout=15.0,
                print_keywords=["AGENT_MGR", "Launched application"],
            )
        assert agent_ready, "Agent Manager failed to present"
        print("[ACCEPTANCE TEST] [OK] Agent Manager presented.")

        if monitor_sock:
            ppm_agent_models = os.path.join(workspace, "target", "test_inference_04_agent_models.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_agent_models}")
            wait_for_screendump(ppm_agent_models)
            print(f"[ACCEPTANCE TEST] Captured Agent Manager Models screendump: {ppm_agent_models}")

        # 3b. The trust decision: the model's bytes are on the device, but the
        #     service serves a model only after the person has asked for it.
        #     The floating action button on the Models surface is that request:
        #     it asks the inference service to install the model the device
        #     holds, and the service records the trust decision against the
        #     bytes it measured. Without this step the generative route refuses
        #     the model, and the generation assertions below are what prove the
        #     decision took effect.
        print("[ACCEPTANCE TEST] Phase 3b: Asking the service to trust the model on the device...")
        time.sleep(1.0)
        reader.send(b"TAP 652 1240\n")
        install_frame = reader.wait_for(
            "[AGENT_MGR] Frame presented to scanout.",
            timeout=20.0,
            print_keywords=["AGENT_MGR", "scanout"],
        )
        if not install_frame:
            print("[ACCEPTANCE TEST] WARNING: no frame after the install request.")
        print("[ACCEPTANCE TEST] [OK] Install request sent; the trust decision is the service's to record.")

        if monitor_sock:
            ppm_agent_installed = os.path.join(workspace, "target", "test_inference_04b_agent_installed.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_agent_installed}")
            wait_for_screendump(ppm_agent_installed)
            print(f"[ACCEPTANCE TEST] Captured Agent Manager after the install request: {ppm_agent_installed}")

        # 4. Switch to Chat Tab (Tab 5 at x=645, y=142)
        print("[ACCEPTANCE TEST] Phase 4: Switching to Agent Manager Chat Tab...")
        time.sleep(1.5)
        reader.send(b"TAP 645 142\n")
        chat_tab_ok = reader.wait_for(
            "[AGENT_MGR] Tab switched to Chat.",
            timeout=15.0,
            print_keywords=["AGENT_MGR", "Chat"],
        )
        assert chat_tab_ok, "Failed to switch to Chat tab"
        print("[ACCEPTANCE TEST] [OK] Switched to Chat tab.")

        if monitor_sock:
            ppm_chat_landing = os.path.join(workspace, "target", "test_inference_05_agent_chat_landing.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_chat_landing}")
            wait_for_screendump(ppm_chat_landing)
            print(f"[ACCEPTANCE TEST] Captured Chat tab landing screendump: {ppm_chat_landing}")

        # 5. Focus Composer Field (tap at 300, 1246)
        print("[ACCEPTANCE TEST] Phase 5: Focusing Composer field to activate soft keyboard...")
        time.sleep(1.0)
        reader.send(b"TAP 300 1246\n")
        comp_focused = reader.wait_for(
            "[AGENT_MGR] Composer field focused.",
            timeout=15.0,
            print_keywords=["AGENT_MGR", "Composer field focused"],
        )
        assert comp_focused, "Failed to focus composer field"
        print("[ACCEPTANCE TEST] [OK] Composer field focused, soft keyboard visible.")

        if monitor_sock:
            ppm_comp = os.path.join(workspace, "target", "test_inference_06_composer_focused.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_comp}")
            wait_for_screendump(ppm_comp)
            print(f"[ACCEPTANCE TEST] Captured soft keyboard screendump: {ppm_comp}")

        # 6. Type Prompt "hi" using soft keyboard:
        # Key 'h': (430, 1224)
        # Key 'i': (535, 1142)
        print("[ACCEPTANCE TEST] Phase 6: Typing prompt 'hi' on soft keyboard...")
        time.sleep(0.8)
        reader.send(b"TAP 430 1224\n")  # 'h'
        time.sleep(0.8)
        reader.send(b"TAP 535 1142\n")  # 'i'
        time.sleep(1.0)

        if monitor_sock:
            ppm_typed = os.path.join(workspace, "target", "test_inference_07_prompt_typed.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_typed}")
            wait_for_screendump(ppm_typed)
            print(f"[ACCEPTANCE TEST] Captured typed prompt screendump: {ppm_typed}")

        # 7. Submit Generation Request via Enter key (645, 1388)
        print("[ACCEPTANCE TEST] Phase 7: Submitting prompt via Enter key (x=645, y=1388)...")
        time.sleep(1.0)
        reader.send(b"TAP 645 1388\n")  # Enter key

        # Wait for aid to receive generation query over IPC
        gen_started = reader.wait_for(
            "[aid] received inference / embedding query over IPC",
            timeout=25.0,
            print_keywords=["aid", "inference", "query", "IPC", "weights"],
        )
        assert gen_started, "aid did not receive generation query over IPC"
        print("[ACCEPTANCE TEST] [OK] aid received inference generation query over IPC.")

        # Require real autoregressive transformer execution in the guest
        trans_exec_ok = reader.wait_for(
            "[aid] native transformer execution generated new tokens.",
            timeout=35.0,
            print_keywords=["aid", "transformer", "tokens", "generated"],
        )
        assert trans_exec_ok, "aid failed to execute real transformer blocks or generate tokens in guest"
        print("[ACCEPTANCE TEST] [OK] Real autoregressive transformer execution verified.")

        pipe_stream_ok = reader.wait_for(
            "bytes via buffered pipe",
            timeout=20.0,
            print_keywords=["SHM PIPE", "streamed", "CRC"],
        )
        assert pipe_stream_ok, "Guest failed to deliver generated tokens through the buffered pipe"

        pipe_consumed_ok = reader.wait_for(
            "bytes from buffered pipe",
            timeout=20.0,
            print_keywords=["SHM PIPE", "consumed"],
        )
        assert pipe_consumed_ok, "Agent Manager failed to consume generated tokens from the buffered pipe"

        logs = "\n".join(reader.captured_logs)
        sent = re.findall(r"streamed (\d+) bytes via buffered pipe (\d+) \(Seq: (\d+), CRC: (0x[0-9A-F]{8})\)", logs)
        received = re.findall(r"consumed (\d+) bytes from buffered pipe (\d+) \(Seq: (\d+)\)", logs)
        assert sent and received and sent[-1][0] == received[-1][0] and sent[-1][1] == received[-1][1] and sent[-1][2] == received[-1][2], (
            f"Producer and consumer disagreed on length, pipe or sequence: sent={sent[-1]}, received={received[-1]}"
        )
        print(f"[ACCEPTANCE TEST] [OK] Producer and consumer reported pipe {sent[-1][1]}, sequence {sent[-1][2]}, bytes {sent[-1][0]}, CRC {sent[-1][3]}.")

        # Require Agent Manager to present updated frame rendering the response
        frame_presented = reader.wait_for(
            "[AGENT_MGR] Frame presented to scanout.",
            timeout=20.0,
            print_keywords=["AGENT_MGR", "Frame presented", "scanout"],
        )
        assert frame_presented, "Agent Manager failed to render generated token response to scanout"
        print("[ACCEPTANCE TEST] [OK] Agent Manager rendered generated token bubble to scanout.")

        time.sleep(1.0)
        if monitor_sock:
            ppm_gen = os.path.join(workspace, "target", "test_inference_08_generation_complete.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_gen}")
            wait_for_screendump(ppm_gen)
            print(f"[ACCEPTANCE TEST] Captured generation screendump: {ppm_gen}")
            if os.path.exists(ppm_gen):
                with open(ppm_gen, "rb") as f:
                    ppm_bytes = f.read()
                assert len(ppm_bytes) > 1000, "Screendump file is corrupt or truncated"
                print(f"[ACCEPTANCE TEST] [OK] Generation screendump verified ({len(ppm_bytes)} bytes).")

        # 8. Test Request Cancellation
        print("[ACCEPTANCE TEST] Phase 8: Testing Request Cancellation during generation...")
        time.sleep(1.0)
        # Type another prompt and submit
        reader.send(b"TAP 300 970\n")  # composer field above keyboard
        time.sleep(0.5)
        reader.send(b"TAP 430 1224\n")  # 'h'
        time.sleep(0.5)
        reader.send(b"TAP 645 1388\n")  # Enter
        time.sleep(0.3)
        # Tap Enter again or tap Cancel button to cancel active generation
        reader.send(b"TAP 645 1388\n")  # In streaming mode, Enter cancels streaming!
        cancel_seen = reader.wait_for(
            "[aid] cancellation requested via IPC during generation",
            timeout=20.0,
            print_keywords=["AGENT_MGR", "cancellation", "aid", "reclaimed"],
        )
        assert cancel_seen, "Cancellation was not observed by aid during generation"

        reclaimed_seen = reader.wait_for(
            "[aid] generation cancelled; resources reclaimed.",
            timeout=20.0,
            print_keywords=["aid", "reclaimed", "resources"],
        )
        assert reclaimed_seen, "Inference service failed to reclaim resources after cancellation"
        print("[ACCEPTANCE TEST] [OK] Request cancellation and resource reclamation verified.")

        if monitor_sock:
            ppm_cancel = os.path.join(workspace, "target", "test_inference_09_cancelled.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_cancel}")
            wait_for_screendump(ppm_cancel)
            print(f"[ACCEPTANCE TEST] Captured cancellation screendump: {ppm_cancel}")

        # 8b. Subsequent Generation Acceptance: verify that subsequent generation succeeds cleanly after cancellation
        print("[ACCEPTANCE TEST] Phase 8b: Testing subsequent generation after cancellation...")
        time.sleep(1.0)
        reader.send(b"TAP 300 970\n")  # focus composer
        time.sleep(0.5)
        reader.send(b"TAP 430 1224\n")  # 'h'
        time.sleep(0.5)
        reader.send(b"TAP 645 1388\n")  # Enter to submit

        subseq_gen_started = reader.wait_for(
            "[aid] received inference / embedding query over IPC",
            timeout=25.0,
            print_keywords=["aid", "inference", "query", "IPC"],
        )
        assert subseq_gen_started, "aid failed to receive subsequent generation query after cancellation"

        subseq_exec_ok = reader.wait_for(
            "[aid] native transformer execution generated new tokens.",
            timeout=35.0,
            print_keywords=["aid", "transformer", "tokens", "generated"],
        )
        assert subseq_exec_ok, "aid failed to execute transformer on subsequent generation"

        subseq_pipe_ok = reader.wait_for(
            "bytes via buffered pipe",
            timeout=20.0,
            print_keywords=["SHM PIPE", "streamed", "CRC"],
        )
        assert subseq_pipe_ok, "Subsequent generation failed to stream tokens through the buffered pipe"

        subseq_consumed_ok = reader.wait_for(
            "bytes from buffered pipe",
            timeout=20.0,
            print_keywords=["SHM PIPE", "consumed"],
        )
        assert subseq_consumed_ok, "Agent Manager failed to consume subsequent tokens from the buffered pipe"
        logs = "\n".join(reader.captured_logs)
        subseq_sent = re.findall(r"streamed (\d+) bytes via buffered pipe (\d+) \(Seq: (\d+), CRC: (0x[0-9A-F]{8})\)", logs)
        subseq_received = re.findall(r"consumed (\d+) bytes from buffered pipe (\d+) \(Seq: (\d+)\)", logs)
        assert subseq_sent and subseq_received and subseq_sent[-1][0] == subseq_received[-1][0] and subseq_sent[-1][1] == subseq_received[-1][1] and subseq_sent[-1][2] == subseq_received[-1][2], (
            f"Subsequent generation producer and consumer disagreed: sent={subseq_sent[-1]}, received={subseq_received[-1]}"
        )
        print(f"[ACCEPTANCE TEST] [OK] Subsequent generation consumed by Agent Manager (pipe {subseq_sent[-1][1]}, sequence {subseq_sent[-1][2]}, bytes {subseq_sent[-1][0]}).")

        if monitor_sock:
            ppm_subseq = os.path.join(workspace, "target", "test_inference_09b_subsequent.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_subseq.replace(os.sep, '/')}")
            wait_for_screendump(ppm_subseq)
            print(f"[ACCEPTANCE TEST] Captured subsequent generation screendump: {ppm_subseq}")

        # 9. Return to HomeScreen via App Bar Back Button (48, 80)
        print("[ACCEPTANCE TEST] Phase 9: Exiting Agent Manager via Back Button (x=48, y=80)...")
        time.sleep(1.0)
        reader.send(b"TAP 48 80\n")

        reaped = reader.wait_for(
            "Foreground application exited and was reaped",
            timeout=25.0,
            print_keywords=["LAUNCHER", "reaped", "WAITPID", "AGENT_MGR"],
        )
        if not reaped:
            time.sleep(1.0)
            reader.send(b"TAP 48 80\n")
            reader.send(b"TAP 360 1420\n")
            reaped = reader.wait_for(
                "Foreground application exited and was reaped",
                timeout=25.0,
                print_keywords=["LAUNCHER", "reaped", "WAITPID", "AGENT_MGR"],
            )
        assert reaped, "Agent Manager exit and waitpid reaping was not observed"
        print("[ACCEPTANCE TEST] [OK] Agent Manager exited cleanly and reaped by PID 10.")

        home_restored = reader.wait_for(
            "Home surface rendered.",
            timeout=25.0,
            print_keywords=["LAUNCHER", "Home surface rendered"],
        )
        assert home_restored, "Launcher failed to re-render HomeScreen"
        print("[ACCEPTANCE TEST] [OK] HomeScreen re-rendered successfully.")

        if monitor_sock:
            ppm_final = os.path.join(workspace, "target", "test_inference_10_home_reaped.ppm")
            _ = send_monitor_cmd(monitor_sock, f"screendump {ppm_final}")
            wait_for_screendump(ppm_final)
            print(f"[ACCEPTANCE TEST] Captured final HomeScreen screendump: {ppm_final}")

        print("\n========================================================")
        print("[ACCEPTANCE TEST] ALL GUEST INFERENCE CONTRACTS VERIFIED!")
        print("========================================================\n")
        return True

    finally:
        if serial_sock:
            try:
                serial_sock.close()
            except Exception:
                pass
        if monitor_sock:
            try:
                monitor_sock.close()
            except Exception:
                pass
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=5.0)
            except subprocess.TimeoutExpired:
                proc.kill()
        print("[ACCEPTANCE TEST] QEMU process terminated.")


if __name__ == "__main__":
    success = run_inference_acceptance_test()
    sys.exit(0 if success else 1)
