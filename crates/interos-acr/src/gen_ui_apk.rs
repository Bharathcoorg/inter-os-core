// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Host contract test: a real app-generated UI.
//!
//! Parses the deterministic fixture APK produced by
//! `tests/fixtures/gen_ui_apk.py`, runs its `MainActivity.onCreate` Dalvik
//! bytecode through the VM view model, and asserts the Activity published a
//! content view carrying the app's own text. This proves the view tree was
//! built by the app's code (new-instance TextView -> setText -> setContentView),
//! not by a placeholder synthesised by the runtime.
//!
//! This is a host test only. It does not claim guest rendering or scanout.

#[cfg(test)]
mod tests {
    use crate::runtime::DalvikVmState;
    use crate::view::ViewType;
    use interos_android_format::apk::ZipArchive;
    use interos_android_format::dex::DexPackage;

    const APP_CLASS: &str = "Lorg/interos/MainActivity;";
    const APP_TEXT: &str = "Hello from app code";

    // Deterministic fixture emitted by `python tests/fixtures/gen_ui_apk.py`.
    static GEN_UI_APK: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures/gen_ui_app.apk"));

    #[test]
    fn app_oncreate_bytecode_builds_the_content_view_tree() {
        // 1. The fixture is a valid PKZip container carrying classes.dex.
        let zip = ZipArchive::parse(GEN_UI_APK).expect("fixture APK must be a valid PKZip container");
        let dex_bytes = zip
            .extract_file("classes.dex")
            .expect("fixture APK must carry classes.dex");

        // 2. The DEX parses under the real production parser (Adler-32 verified).
        let dex = DexPackage::parse(&dex_bytes).expect("fixture classes.dex must parse");

        // 3. Nothing is published until the app's own code runs.
        let mut vm = DalvikVmState::new();
        assert_eq!(vm.content_view_id(), None);
        assert!(vm.content_view().is_none());

        // 4. Execute the app's onCreate bytecode. Its invoke-* instructions route
        //    through dispatch_framework_method, which records the real view state.
        let ret = vm.invoke_method(&dex, APP_CLASS, "onCreate");
        assert_eq!(ret, Ok(0), "onCreate must run to completion");

        // 5. The Activity now has a content view built by app code.
        let view = vm
            .content_view()
            .expect("app onCreate must publish a content view through the VM view model");

        // 6. The root is the app's TextView carrying the app's own text.
        match view.view_type {
            ViewType::TextView { ref text, .. } => {
                assert_eq!(text, APP_TEXT, "content view must carry the app's own text");
            }
            other => panic!("expected an app-built TextView root, got {other:?}"),
        }
    }
}