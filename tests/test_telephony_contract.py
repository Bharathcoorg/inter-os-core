from pathlib import Path

ROOT = Path(__file__).parents[1]
modem = (ROOT / "kernel/src/telephony/modem.rs").read_text()
ril = (ROOT / "kernel/src/telephony/serial_ril.rs").read_text()
assert "MAX_PHONE_NUMBER_LEN: usize = 32" in modem
assert "MAX_SMS_BODY_LEN: usize = 1024" in modem
assert "MAX_SMS_RECORDS: usize = 32" in modem
assert "sms_outbox.len() >= MAX_SMS_RECORDS" in modem
assert "sms_inbox.len() >= MAX_SMS_RECORDS" in modem
assert "URC_QUEUE_CAPACITY: usize = 32" in ril
assert "full_cmd.len() > MAX_AT_LINE_LEN" in ril
assert "fn push_urc(&mut self, event: UrcEvent)" in ril
print("M37 telephony boundary contract: PASS")
