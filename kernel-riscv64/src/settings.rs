// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! RISC-V64 In-Kernel Persistent Settings Key-Value Store.

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

const MAX_SETTINGS_BYTES: usize = 4096;

pub struct SettingsStore {
    data: Vec<u8>,
}

impl SettingsStore {
    pub const fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn init_defaults(&mut self) {
        if self.data.is_empty() {
            let defaults = b"display.brightness=80\nsystem.theme=dark\naudio.volume=70\nabout.os_version=InterOS 0.1.0 (RISC-V64)\npower.profile=balanced\nowner.name=InterOS User\n";
            self.data.extend_from_slice(defaults);
        }
    }

    fn find(&self, key: &str) -> Option<(usize, usize)> {
        let mut start = 0;
        while start < self.data.len() {
            let end = self.data[start..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|x| start + x)
                .unwrap_or(self.data.len());
            let line = &self.data[start..end];
            if let Some(eq) = line.iter().position(|b| *b == b'=') {
                if core::str::from_utf8(&line[..eq]).ok() == Some(key) {
                    return Some((start, end));
                }
            }
            start = end.saturating_add(1);
        }
        None
    }

    pub fn get(&self, key: &str, out: &mut [u8]) -> usize {
        let Some((start, end)) = self.find(key) else {
            return 0;
        };
        let line = &self.data[start..end];
        let eq = line.iter().position(|b| *b == b'=').unwrap_or(line.len());
        let value = &line[eq.saturating_add(1)..];
        let n = value.len().min(out.len());
        out[..n].copy_from_slice(&value[..n]);
        n
    }

    pub fn set(&mut self, key: &str, value: &[u8]) -> Result<(), ()> {
        if key.is_empty()
            || key.len() > 96
            || value.len() > 512
            || key.as_bytes().iter().any(|b| *b == b'\n' || *b == b'=')
            || value.iter().any(|b| *b == b'\n')
        {
            return Err(());
        }
        let replacement_len = key.len() + 1 + value.len() + 1;
        let old = self.find(key);
        let mut next = Vec::with_capacity(MAX_SETTINGS_BYTES);
        let mut pos = 0;
        let mut replaced = false;
        while pos < self.data.len() {
            let end = self.data[pos..]
                .iter()
                .position(|b| *b == b'\n')
                .map(|x| pos + x)
                .unwrap_or(self.data.len());
            let is_old = old.map(|(s, e)| s == pos && e == end).unwrap_or(false);
            if !is_old {
                if next.len() + end - pos + 1 > MAX_SETTINGS_BYTES {
                    return Err(());
                }
                next.extend_from_slice(&self.data[pos..end]);
                next.push(b'\n');
            } else {
                if next.len() + replacement_len > MAX_SETTINGS_BYTES {
                    return Err(());
                }
                next.extend_from_slice(key.as_bytes());
                next.push(b'=');
                next.extend_from_slice(value);
                next.push(b'\n');
                replaced = true;
            }
            pos = end.saturating_add(1);
        }
        if !replaced {
            if next.len() + replacement_len > MAX_SETTINGS_BYTES {
                return Err(());
            }
            next.extend_from_slice(key.as_bytes());
            next.push(b'=');
            next.extend_from_slice(value);
            next.push(b'\n');
        }
        self.data = next;
        Ok(())
    }
}

pub static SETTINGS: Mutex<SettingsStore> = Mutex::new(SettingsStore::new());
