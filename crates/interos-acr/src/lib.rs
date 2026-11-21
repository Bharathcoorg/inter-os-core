// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Unprivileged Ring-3 Android Compatibility Runtime (ACR) for Inter OS

#![no_std]
extern crate alloc;

pub mod runtime;
pub mod looper;
pub mod view;
pub mod bionic;
pub mod jni;
pub mod framework;
pub mod binder;
pub mod vfs;
pub mod bridge;
pub mod draw;
pub mod manager;
