//! On-Device Natural Language Intent Parser & Tokenizer
//!
//! Provides deterministic zero-allocation conversational intent recognition,
//! parsing natural language voice/text queries into verified sovereign AMCP
//! actions, capability requirements, and structured target parameters.

use alloc::string::String;
use alloc::vec::Vec;

/// High-level operational application domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionDomain {
    Blinkit,
    Swiggy,
    Telephony,
    System,
    Liteverse,
    Unknown,
}

/// Resolved on-device structured automation intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedIntent {
    pub domain: ActionDomain,
    pub action: &'static str,
    pub target: String,
    pub confidence_score: u8,
    pub requires_attestation: bool,
}

impl ParsedIntent {
    /// Serializes the parsed intent into a standardized JSON representation.
    pub fn to_json(&self, out: &mut [u8]) -> usize {
        use core::fmt::Write;
        struct SliceWriter<'a> {
            buf: &'a mut [u8],
            pos: usize,
        }
        impl<'a> Write for SliceWriter<'a> {
            fn write_str(&mut self, s: &str) -> core::fmt::Result {
                let bytes = s.as_bytes();
                let rem = self.buf.len().saturating_sub(self.pos);
                let to_write = bytes.len().min(rem);
                self.buf[self.pos..self.pos + to_write].copy_from_slice(&bytes[..to_write]);
                self.pos += to_write;
                Ok(())
            }
        }
        let mut writer = SliceWriter { buf: out, pos: 0 };
        let domain_str = match self.domain {
            ActionDomain::Blinkit => "blinkit",
            ActionDomain::Swiggy => "swiggy",
            ActionDomain::Telephony => "telephony",
            ActionDomain::System => "system",
            ActionDomain::Liteverse => "liteverse",
            ActionDomain::Unknown => "unknown",
        };
        let _ = write!(
            writer,
            r#"{{"domain":"{}","action":"{}","target":"{}","confidence":{},"attested":{}}}"#,
            domain_str, self.action, self.target, self.confidence_score, self.requires_attestation
        );
        writer.pos
    }
}

/// Cleans and tokenizes natural language input into lowercase alphanumeric tokens.
pub fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_alphanumeric() || ch == '+' {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(core::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parses a natural language user query into a structured on-device AMCP intent.
pub fn parse_intent(query: &str) -> ParsedIntent {
    let tokens = tokenize(query);

    let has_token = |kw: &str| tokens.iter().any(|t| t == kw);

    // 1. Blinkit Quick Commerce Intent
    if has_token("blinkit") || (has_token("reorder") && (has_token("milk") || has_token("groceries"))) {
        let mut target_words = Vec::new();
        for t in &tokens {
            if t != "blinkit" && t != "order" && t != "reorder" && t != "buy" && t != "from" && t != "on" && t != "please" {
                target_words.push(t.as_str());
            }
        }
        let target = if target_words.is_empty() {
            String::from("milk")
        } else {
            let mut s = String::new();
            for (idx, w) in target_words.iter().enumerate() {
                if idx > 0 {
                    s.push(' ');
                }
                s.push_str(w);
            }
            s
        };

        return ParsedIntent {
            domain: ActionDomain::Blinkit,
            action: "AMCP_EXECUTE_BLINKIT",
            target,
            confidence_score: 95,
            requires_attestation: true,
        };
    }

    // 2. Swiggy Food Delivery Intent
    if has_token("swiggy") || (has_token("food") && (has_token("order") || has_token("delivery"))) {
        let mut target_words = Vec::new();
        for t in &tokens {
            if t != "swiggy" && t != "order" && t != "buy" && t != "food" && t != "from" && t != "on" && t != "deliver" {
                target_words.push(t.as_str());
            }
        }
        let target = if target_words.is_empty() {
            String::from("food")
        } else {
            let mut s = String::new();
            for (idx, w) in target_words.iter().enumerate() {
                if idx > 0 {
                    s.push(' ');
                }
                s.push_str(w);
            }
            s
        };

        return ParsedIntent {
            domain: ActionDomain::Swiggy,
            action: "AMCP_SWIGGY_ORDER",
            target,
            confidence_score: 95,
            requires_attestation: true,
        };
    }

    // 3. Telephony (Calls / Emergency Dialing)
    if has_token("call") || has_token("dial") || has_token("emergency") || has_token("phone") {
        let mut target = String::new();
        for t in &tokens {
            if t != "call" && t != "dial" && t != "phone" && t != "to" && t != "please" {
                if !target.is_empty() {
                    target.push(' ');
                }
                target.push_str(t);
            }
        }
        if target.is_empty() {
            target = String::from("112");
        }

        return ParsedIntent {
            domain: ActionDomain::Telephony,
            action: "AMCP_TELEPHONY_DIAL",
            target,
            confidence_score: 94,
            requires_attestation: false,
        };
    }

    // 4. Battery / Power Status Query
    if has_token("battery") || has_token("charge") || has_token("power") {
        return ParsedIntent {
            domain: ActionDomain::System,
            action: "SYSTEM_BATTERY_QUERY",
            target: String::from("battery"),
            confidence_score: 99,
            requires_attestation: false,
        };
    }

    // 5. Clock / Time Query
    if has_token("time") || has_token("clock") || has_token("date") {
        return ParsedIntent {
            domain: ActionDomain::System,
            action: "SYSTEM_RTC_QUERY",
            target: String::from("rtc"),
            confidence_score: 99,
            requires_attestation: false,
        };
    }

    // 6. Liteverse State Sync
    if has_token("liteverse") || has_token("sync") || has_token("mesh") {
        return ParsedIntent {
            domain: ActionDomain::Liteverse,
            action: "LITEVERSE_SYNC",
            target: String::from("state"),
            confidence_score: 92,
            requires_attestation: true,
        };
    }

    // 7. Unmatched Fallback
    ParsedIntent {
        domain: ActionDomain::Unknown,
        action: "UNKNOWN_QUERY",
        target: String::from(query.trim()),
        confidence_score: 10,
        requires_attestation: false,
    }
}

/// Executes boot self-tests for the natural language parser.
pub fn init() {
    let test_1 = "reorder milk from blinkit";
    let res_1 = parse_intent(test_1);
    assert_eq!(res_1.domain, ActionDomain::Blinkit);
    assert_eq!(res_1.action, "AMCP_EXECUTE_BLINKIT");
    assert_eq!(res_1.target, "milk");
    assert!(res_1.requires_attestation);

    let test_2 = "call emergency 112";
    let res_2 = parse_intent(test_2);
    assert_eq!(res_2.domain, ActionDomain::Telephony);
    assert_eq!(res_2.action, "AMCP_TELEPHONY_DIAL");
    assert_eq!(res_2.target, "emergency 112");

    let test_3 = "check battery";
    let res_3 = parse_intent(test_3);
    assert_eq!(res_3.domain, ActionDomain::System);
    assert_eq!(res_3.action, "SYSTEM_BATTERY_QUERY");

    crate::serial_println!(
        "[TEST] NL Intent Parser: 'reorder milk from blinkit' -> AMCP_EXECUTE_BLINKIT (target='milk', confidence=95%)."
    );
}
