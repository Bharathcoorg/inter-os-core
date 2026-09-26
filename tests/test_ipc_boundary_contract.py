from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SECURE = (ROOT / "kernel/src/ipc/secure_channel.rs").read_text()
SHM = (ROOT / "kernel/src/ipc/shm_pipe.rs").read_text()


def must(text: str, needle: str, label: str) -> None:
    assert needle in text, f"missing {label}: {needle}"


def main() -> None:
    must(SECURE, "pub const MAX_IPC_PAYLOAD: usize = 64;", "secure IPC payload bound")
    must(SECURE, "frame.payload_len as usize > self::MAX_IPC_PAYLOAD", "incoming frame length validation")
    must(SECURE, "return Err(SecureIpcError::PayloadTooLarge);", "oversized secure frame rejection")

    must(SHM, "if idx >= self.pipes.len()", "transfer/receive pipe index validation")
    must(SHM, "let idx = pipe_id as usize;", "bounded pipe index conversion")
    must(SHM, "return Err(ShmError::PipeNotFound);", "invalid pipe rejection")
    must(SHM, "if idx >= self.pipes.len() {\n            return None;", "stats pipe index validation")

    assert "let idx = (pipe_id as usize) % self.pipes.len();" not in SHM, "pipe APIs must not alias invalid IDs through modulo"
    print("IPC boundary contract: PASS")


if __name__ == "__main__":
    main()
