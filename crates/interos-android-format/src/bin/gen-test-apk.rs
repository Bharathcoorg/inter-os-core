// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

use std::env;
use std::fs;
use interos_android_format::apk::{
    create_signed_test_apk, create_signed_test_apk_with_cert, create_signed_ui_test_apk,
};

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: gen_test_apk <package_name> <out_path> [--ui-text <text> | cert_str]");
        std::process::exit(1);
    }
    let pkg = &args[1];
    let out_path = &args[2];
    let apk_bytes = if args.len() >= 5 && args[3] == "--ui-text" {
        create_signed_ui_test_apk(pkg, &["android.permission.INTERNET"], &args[4])
    } else if args.len() > 3 {
        create_signed_test_apk_with_cert(pkg, &["android.permission.INTERNET"], args[3].as_bytes())
    } else {
        create_signed_test_apk(pkg, &["android.permission.INTERNET"])
    };
    fs::write(out_path, apk_bytes).expect("write apk file");
    println!("Generated APK for {} at {}", pkg, out_path);
}
