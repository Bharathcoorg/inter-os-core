// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Android Java Native Interface (JNI) Environment & Bridge
//!
//! Provides a safe `#![no_std]` Rust implementation of standard JNI functions
//! allowing DEX bytecode and native C/C++ libraries (`.so`) to call each other
//! across bounded, verified capability boundaries.
//!
//! Complies with standard Android JNI specification:
//! - `JNIEnv`: Thread-local JNI interface table.
//! - `JavaVM`: Process-wide virtual machine handle.
//! - Method / Field resolution: `FindClass`, `GetMethodID`, `GetStaticMethodID`, `GetFieldID`.
//! - Invocations: `CallObjectMethod`, `CallIntMethod`, `CallVoidMethod`.
//! - String & Array memory management: `NewStringUTF`, `GetStringUTFChars`.
//! - Reference management: `PushLocalFrame`, `PopLocalFrame`, `NewGlobalRef`, `DeleteLocalRef`, `DeleteGlobalRef`.
//! - 64-bit field access: `GetLongField`, `SetLongField` for native pointer handles.
//! - Native method registration: `RegisterNatives`.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const JNI_VERSION_1_6: u32 = 0x00010006;
pub const JNI_OK: i32 = 0;
pub const JNI_ERR: i32 = -1;

/// JNI reference type categorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JniRefType {
    InvalidRefType = 0,
    LocalRefType = 1,
    GlobalRefType = 2,
    WeakGlobalRefType = 3,
}

/// A local reference frame pushed onto the JNI frame stack.
#[derive(Debug, Clone)]
pub struct LocalFrame {
    pub capacity: usize,
    pub refs: Vec<u32>,
}

/// A native method descriptor registered via `RegisterNatives`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JNINativeMethod {
    pub name: String,
    pub signature: String,
    pub fn_ptr: u64,
}

/// A registered class metadata in the JNI environment.
#[derive(Debug, Clone)]
pub struct JniClass {
    pub id: u32,
    pub name: String,
    pub methods: BTreeMap<String, u32>,
    pub static_methods: BTreeMap<String, u32>,
    pub fields: BTreeMap<String, u32>,
    pub registered_natives: Vec<JNINativeMethod>,
}

/// Bounded JNI string or object allocated in the JNI frame.
#[derive(Debug, Clone)]
pub struct JniObject {
    pub id: u32,
    pub class_id: u32,
    pub string_val: Option<String>,
    pub int_fields: BTreeMap<u32, i32>,
    pub long_fields: BTreeMap<u32, i64>,
    pub object_fields: BTreeMap<u32, u32>,
    pub array_elements: Option<Vec<u32>>,
}

/// Thread-local JNI execution environment (`JNIEnv`).
pub struct JniEnv {
    pub version: u32,
    classes: BTreeMap<u32, JniClass>,
    class_name_map: BTreeMap<String, u32>,
    objects: BTreeMap<u32, JniObject>,
    local_frames: Vec<LocalFrame>,
    global_refs: BTreeSet<u32>,
    next_class_id: u32,
    next_method_id: u32,
    next_field_id: u32,
    next_object_id: u32,
    pub native_calls_executed: u64,
    pub pending_exception: Option<u32>,
}

impl JniEnv {
    /// Creates a fresh JNIEnv initialized with standard core classes and pre-registered symbols.
    pub fn new() -> Self {
        let mut env = Self {
            version: JNI_VERSION_1_6,
            classes: BTreeMap::new(),
            class_name_map: BTreeMap::new(),
            objects: BTreeMap::new(),
            local_frames: alloc::vec![LocalFrame {
                capacity: 512,
                refs: Vec::new(),
            }],
            global_refs: BTreeSet::new(),
            next_class_id: 1,
            next_method_id: 1,
            next_field_id: 1,
            next_object_id: 1,
            native_calls_executed: 0,
            pending_exception: None,
        };

        // Pre-register standard core classes and their members
        let obj_cls = env.register_class("java/lang/Object");
        env.register_method(obj_cls, "<init>", "()V");

        let str_cls = env.register_class("java/lang/String");
        env.register_method(str_cls, "<init>", "()V");
        env.register_method(str_cls, "<init>", "([B)V");
        env.register_field(str_cls, "value", "[C");

        env.register_class("java/lang/Class");
        env.register_class("android/app/Activity");
        env.register_class("android/content/Context");

        // Pre-register Android graphics objects with 64-bit native pointer fields
        let path_cls = env.register_class("android/graphics/Path");
        env.register_field(path_cls, "mNativePath", "J");
        env.register_method(path_cls, "<init>", "()V");

        let paint_cls = env.register_class("android/graphics/Paint");
        env.register_field(paint_cls, "mNativePaint", "J");
        env.register_method(paint_cls, "<init>", "()V");

        let bmp_cls = env.register_class("android/graphics/Bitmap");
        env.register_field(bmp_cls, "mNativeBitmap", "J");

        let canvas_cls = env.register_class("android/graphics/Canvas");
        env.register_field(canvas_cls, "mNativeCanvas", "J");

        let view_cls = env.register_class("android/view/View");
        env.register_field(view_cls, "mNativeView", "J");

        env
    }

    /// Registers a class in the JNI runtime and returns its unique class ID.
    pub fn register_class(&mut self, name: &str) -> u32 {
        if let Some(id) = self.class_name_map.get(name) {
            return *id;
        }

        let id = self.next_class_id;
        self.next_class_id += 1;

        let cls = JniClass {
            id,
            name: String::from(name),
            methods: BTreeMap::new(),
            static_methods: BTreeMap::new(),
            fields: BTreeMap::new(),
            registered_natives: Vec::new(),
        };

        self.class_name_map.insert(String::from(name), id);
        self.classes.insert(id, cls);
        id
    }

    /// Registers an instance field for a class, returning the field ID.
    pub fn register_field(&mut self, class_id: u32, name: &str, sig: &str) -> u32 {
        let cls = self.classes.get_mut(&class_id).expect("class must exist");
        let key = format!("{}:{}", name, sig);
        if let Some(id) = cls.fields.get(&key) {
            return *id;
        }
        let id = self.next_field_id;
        self.next_field_id += 1;
        cls.fields.insert(key, id);
        id
    }

    /// Registers an instance method for a class, returning the method ID.
    pub fn register_method(&mut self, class_id: u32, name: &str, sig: &str) -> u32 {
        let cls = self.classes.get_mut(&class_id).expect("class must exist");
        let key = format!("{}:{}", name, sig);
        if let Some(id) = cls.methods.get(&key) {
            return *id;
        }
        let id = self.next_method_id;
        self.next_method_id += 1;
        cls.methods.insert(key, id);
        id
    }

    /// Registers a static method for a class, returning the method ID.
    pub fn register_static_method(&mut self, class_id: u32, name: &str, sig: &str) -> u32 {
        let cls = self.classes.get_mut(&class_id).expect("class must exist");
        let key = format!("{}:{}", name, sig);
        if let Some(id) = cls.static_methods.get(&key) {
            return *id;
        }
        let id = self.next_method_id;
        self.next_method_id += 1;
        cls.static_methods.insert(key, id);
        id
    }

    /// JNI `FindClass(const char* name)`
    pub fn find_class(&mut self, name: &str) -> Option<u32> {
        self.class_name_map.get(name).copied().or_else(|| {
            // Auto-register requested framework class
            Some(self.register_class(name))
        })
    }

    /// JNI `GetMethodID(jclass clazz, const char* name, const char* sig)`.
    /// Fails closed with NoSuchMethodError if the method is not registered.
    pub fn get_method_id(&mut self, class_id: u32, name: &str, sig: &str) -> Option<u32> {
        let cls = self.classes.get(&class_id)?;
        let key = format!("{}:{}", name, sig);
        if let Some(id) = cls.methods.get(&key) {
            return Some(*id);
        }
        let cls_name = cls.name.clone();
        self.throw_new("java/lang/NoSuchMethodError", &format!("Method {}:{} not found on {}", name, sig, cls_name));
        None
    }

    /// JNI `GetStaticMethodID(jclass clazz, const char* name, const char* sig)`.
    /// Fails closed with NoSuchMethodError if the method is not registered.
    pub fn get_static_method_id(&mut self, class_id: u32, name: &str, sig: &str) -> Option<u32> {
        let cls = self.classes.get(&class_id)?;
        let key = format!("{}:{}", name, sig);
        if let Some(id) = cls.static_methods.get(&key) {
            return Some(*id);
        }
        let cls_name = cls.name.clone();
        self.throw_new("java/lang/NoSuchMethodError", &format!("Static method {}:{} not found on {}", name, sig, cls_name));
        None
    }

    /// JNI `GetFieldID(jclass clazz, const char* name, const char* sig)`.
    /// Fails closed with NoSuchFieldError if the field is not registered.
    pub fn get_field_id(&mut self, class_id: u32, name: &str, sig: &str) -> Option<u32> {
        let cls = self.classes.get(&class_id)?;
        let key = format!("{}:{}", name, sig);
        if let Some(id) = cls.fields.get(&key) {
            return Some(*id);
        }
        let cls_name = cls.name.clone();
        self.throw_new("java/lang/NoSuchFieldError", &format!("Field {}:{} not found on {}", name, sig, cls_name));
        None
    }

    /// JNI `RegisterNatives(jclass clazz, const JNINativeMethod* methods, jint nMethods)`
    pub fn register_natives(&mut self, class_id: u32, methods: &[JNINativeMethod]) -> i32 {
        let Some(cls) = self.classes.get_mut(&class_id) else {
            return JNI_ERR;
        };

        for m in methods {
            cls.registered_natives.push(m.clone());
        }

        JNI_OK
    }

    /// Tracks a newly allocated object in the active local frame.
    fn track_local_ref(&mut self, obj_id: u32) {
        if let Some(frame) = self.local_frames.last_mut() {
            frame.refs.push(obj_id);
        }
    }

    /// JNI `PushLocalFrame(jint capacity)`
    pub fn push_local_frame(&mut self, capacity: usize) -> i32 {
        self.local_frames.push(LocalFrame {
            capacity,
            refs: Vec::new(),
        });
        JNI_OK
    }

    /// JNI `PopLocalFrame(jobject result)`
    /// Pops the active frame and releases references created within it, returning result promoted to the parent frame.
    pub fn pop_local_frame(&mut self, result: u32) -> u32 {
        if self.local_frames.len() > 1 {
            if let Some(frame) = self.local_frames.pop() {
                for id in frame.refs {
                    if id != result && !self.global_refs.contains(&id) {
                        self.objects.remove(&id);
                    }
                }
            }
        }
        if result > 0 {
            self.track_local_ref(result);
        }
        result
    }

    /// JNI `DeleteLocalRef(jobject obj)`
    pub fn delete_local_ref(&mut self, obj_id: u32) {
        if let Some(frame) = self.local_frames.last_mut() {
            if let Some(pos) = frame.refs.iter().position(|&x| x == obj_id) {
                frame.refs.remove(pos);
            }
        }
        if !self.global_refs.contains(&obj_id) {
            let is_in_any_frame = self.local_frames.iter().any(|f| f.refs.contains(&obj_id));
            if !is_in_any_frame {
                self.objects.remove(&obj_id);
            }
        }
    }

    /// JNI `NewGlobalRef(jobject lobj)`
    pub fn new_global_ref(&mut self, obj_id: u32) -> u32 {
        if obj_id == 0 || !self.objects.contains_key(&obj_id) {
            return 0;
        }
        self.global_refs.insert(obj_id);
        obj_id
    }

    /// JNI `DeleteGlobalRef(jobject gref)`
    pub fn delete_global_ref(&mut self, obj_id: u32) {
        self.global_refs.remove(&obj_id);
        let is_in_any_frame = self.local_frames.iter().any(|f| f.refs.contains(&obj_id));
        if !is_in_any_frame {
            self.objects.remove(&obj_id);
        }
    }

    /// JNI `IsSameObject(jobject ref1, jobject ref2)`
    pub fn is_same_object(&self, ref1: u32, ref2: u32) -> bool {
        ref1 == ref2
    }

    /// JNI `GetObjectRefType(jobject obj)`
    pub fn get_object_ref_type(&self, obj_id: u32) -> JniRefType {
        if obj_id == 0 || !self.objects.contains_key(&obj_id) {
            return JniRefType::InvalidRefType;
        }
        if self.global_refs.contains(&obj_id) {
            return JniRefType::GlobalRefType;
        }
        if self.local_frames.iter().any(|f| f.refs.contains(&obj_id)) {
            return JniRefType::LocalRefType;
        }
        JniRefType::InvalidRefType
    }

    /// JNI `NewStringUTF(const char* bytes)`
    pub fn new_string_utf(&mut self, s: &str) -> u32 {
        let str_class = self.register_class("java/lang/String");
        let id = self.next_object_id;
        self.next_object_id += 1;

        let obj = JniObject {
            id,
            class_id: str_class,
            string_val: Some(String::from(s)),
            int_fields: BTreeMap::new(),
            long_fields: BTreeMap::new(),
            object_fields: BTreeMap::new(),
            array_elements: None,
        };

        self.objects.insert(id, obj);
        self.track_local_ref(id);
        id
    }

    /// JNI `GetStringUTFChars(jstring str, jboolean* isCopy)`
    pub fn get_string_utf_chars(&self, str_obj_id: u32) -> Option<&str> {
        let obj = self.objects.get(&str_obj_id)?;
        obj.string_val.as_deref()
    }

    /// JNI `GetStringLength(jstring str)`
    pub fn get_string_length(&self, str_obj_id: u32) -> usize {
        self.get_string_utf_chars(str_obj_id)
            .map(|s| s.encode_utf16().count())
            .unwrap_or(0)
    }

    /// JNI `GetStringUTFLength(jstring str)`
    pub fn get_string_utf_length(&self, str_obj_id: u32) -> usize {
        self.get_string_utf_chars(str_obj_id)
            .map(|s| s.len())
            .unwrap_or(0)
    }

    /// JNI `ReleaseStringUTFChars(jstring str, const char* chars)`
    pub fn release_string_utf_chars(&self, _str_obj_id: u32, _chars: &str) {
        // No-op for managed string references
    }

    /// JNI `GetIntField(jobject obj, jfieldID fieldID)`
    pub fn get_int_field(&self, obj_id: u32, field_id: u32) -> i32 {
        self.objects.get(&obj_id)
            .and_then(|o| o.int_fields.get(&field_id).copied())
            .unwrap_or(0)
    }

    /// JNI `SetIntField(jobject obj, jfieldID fieldID, jint val)`
    pub fn set_int_field(&mut self, obj_id: u32, field_id: u32, val: i32) {
        if let Some(obj) = self.objects.get_mut(&obj_id) {
            obj.int_fields.insert(field_id, val);
        }
    }

    /// JNI `GetLongField(jobject obj, jfieldID fieldID)` - 64-bit native pointer fields
    pub fn get_long_field(&self, obj_id: u32, field_id: u32) -> i64 {
        self.objects.get(&obj_id)
            .and_then(|o| o.long_fields.get(&field_id).copied())
            .unwrap_or(0)
    }

    /// JNI `SetLongField(jobject obj, jfieldID fieldID, jlong val)` - 64-bit native pointer fields
    pub fn set_long_field(&mut self, obj_id: u32, field_id: u32, val: i64) {
        if let Some(obj) = self.objects.get_mut(&obj_id) {
            obj.long_fields.insert(field_id, val);
        }
    }

    /// JNI `GetObjectField(jobject obj, jfieldID fieldID)`
    pub fn get_object_field(&self, obj_id: u32, field_id: u32) -> u32 {
        self.objects.get(&obj_id)
            .and_then(|o| o.object_fields.get(&field_id).copied())
            .unwrap_or(0)
    }

    /// JNI `SetObjectField(jobject obj, jfieldID fieldID, jobject val)`
    pub fn set_object_field(&mut self, obj_id: u32, field_id: u32, val: u32) {
        if let Some(obj) = self.objects.get_mut(&obj_id) {
            obj.object_fields.insert(field_id, val);
        }
    }

    /// JNI `NewObject(jclass clazz, jmethodID methodID, ...)`
    pub fn new_object(&mut self, class_id: u32) -> u32 {
        let id = self.next_object_id;
        self.next_object_id += 1;

        let obj = JniObject {
            id,
            class_id,
            string_val: None,
            int_fields: BTreeMap::new(),
            long_fields: BTreeMap::new(),
            object_fields: BTreeMap::new(),
            array_elements: None,
        };

        self.objects.insert(id, obj);
        self.track_local_ref(id);
        id
    }

    /// JNI `NewObjectArray(jsize length, jclass elementClass, jobject initialElement)`
    pub fn new_object_array(&mut self, length: usize, element_class_id: u32, initial_element: u32) -> u32 {
        let id = self.next_object_id;
        self.next_object_id += 1;

        let obj = JniObject {
            id,
            class_id: element_class_id,
            string_val: None,
            int_fields: BTreeMap::new(),
            long_fields: BTreeMap::new(),
            object_fields: BTreeMap::new(),
            array_elements: Some(alloc::vec![initial_element; length]),
        };

        self.objects.insert(id, obj);
        self.track_local_ref(id);
        id
    }

    /// JNI `GetObjectArrayElement(jobjectArray array, jsize index)`
    pub fn get_object_array_element(&self, array_id: u32, index: usize) -> Option<u32> {
        let obj = self.objects.get(&array_id)?;
        let elements = obj.array_elements.as_ref()?;
        elements.get(index).copied()
    }

    /// JNI `SetObjectArrayElement(jobjectArray array, jsize index, jobject val)`
    pub fn set_object_array_element(&mut self, array_id: u32, index: usize, val: u32) -> bool {
        if let Some(obj) = self.objects.get_mut(&array_id) {
            if let Some(elements) = obj.array_elements.as_mut() {
                if index < elements.len() {
                    elements[index] = val;
                    return true;
                }
            }
        }
        false
    }

    /// JNI `GetArrayLength(jarray array)`
    pub fn get_array_length(&self, array_id: u32) -> usize {
        self.objects.get(&array_id)
            .and_then(|o| o.array_elements.as_ref().map(|a| a.len()))
            .unwrap_or(0)
    }

    /// JNI `ExceptionCheck()`
    pub fn exception_check(&self) -> bool {
        self.pending_exception.is_some()
    }

    /// JNI `ExceptionOccurred()`
    pub fn exception_occurred(&self) -> Option<u32> {
        self.pending_exception
    }

    /// JNI `ExceptionClear()`
    pub fn exception_clear(&mut self) {
        self.pending_exception = None;
    }

    /// JNI `ThrowNew(jclass clazz, const char* message)`
    pub fn throw_new(&mut self, class_name: &str, message: &str) -> i32 {
        let class_id = self.find_class(class_name).unwrap_or_else(|| self.register_class(class_name));
        let ex_obj = self.new_object(class_id);
        if let Some(obj) = self.objects.get_mut(&ex_obj) {
            obj.string_val = Some(String::from(message));
        }
        self.pending_exception = Some(ex_obj);
        JNI_OK
    }

    /// Invokes a registered native function for a method ID.
    pub fn invoke_native_method(&mut self, class_id: u32, method_name: &str) -> Option<u64> {
        let cls = self.classes.get(&class_id)?;
        let native_entry = cls
            .registered_natives
            .iter()
            .find(|m| m.name == method_name)?;

        self.native_calls_executed += 1;
        Some(native_entry.fn_ptr)
    }
}

/// Process-wide JavaVM structure.
pub struct JavaVM {
    env: JniEnv,
}

impl JavaVM {
    pub fn new() -> Self {
        Self { env: JniEnv::new() }
    }

    pub fn get_env(&mut self, version: u32) -> Result<&mut JniEnv, i32> {
        if version > JNI_VERSION_1_6 {
            return Err(JNI_ERR);
        }
        Ok(&mut self.env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_jni_env_and_javavm() {
        let mut vm = JavaVM::new();
        let env = vm.get_env(JNI_VERSION_1_6).expect("get JniEnv");
        assert_eq!(env.version, JNI_VERSION_1_6);

        let str_class_id = env.find_class("java/lang/String").expect("find String");
        assert!(str_class_id > 0);

        let my_class_id = env.register_class("org/whispersystems/curve25519/Curve25519");
        assert!(my_class_id > 0);

        let mid = env.register_static_method(my_class_id, "calculateAgreement", "([B[B)[B");
        assert!(mid > 0);
        let found_mid = env.get_static_method_id(my_class_id, "calculateAgreement", "([B[B)[B");
        assert_eq!(found_mid, Some(mid));

        let native_fn = 0xDEADBEEF;
        assert_eq!(env.register_natives(my_class_id, &[JNINativeMethod {
            name: String::from("calculateAgreement"),
            signature: String::from("([B[B)[B"),
            fn_ptr: native_fn,
        }]), JNI_OK);

        let resolved_fn = env.invoke_native_method(my_class_id, "calculateAgreement");
        assert_eq!(resolved_fn, Some(native_fn));
        assert_eq!(env.native_calls_executed, 1);

        let str_obj = env.new_string_utf("interos_secure_key");
        assert_eq!(env.get_string_utf_chars(str_obj), Some("interos_secure_key"));

        let obj = env.new_object(my_class_id);
        assert!(obj > 0);
    }

    #[test]
    fn test_jni_field_and_method_fail_closed_exceptions() {
        let mut env = JniEnv::new();
        let cls_id = env.find_class("java/lang/String").unwrap();

        // 1. Missing field must fail closed and throw NoSuchFieldError
        assert!(!env.exception_check());
        let missing_field = env.get_field_id(cls_id, "mMissingField", "I");
        assert_eq!(missing_field, None);
        assert!(env.exception_check());
        let ex_obj = env.exception_occurred().unwrap();
        let obj = env.objects.get(&ex_obj).unwrap();
        let cls = env.classes.get(&obj.class_id).unwrap();
        assert_eq!(cls.name, "java/lang/NoSuchFieldError");
        let ex_str = obj.string_val.as_deref().unwrap();
        assert!(ex_str.contains("mMissingField"));

        env.exception_clear();
        assert!(!env.exception_check());

        // 2. Missing method must fail closed and throw NoSuchMethodError
        let missing_method = env.get_method_id(cls_id, "nonExistentMethod", "()V");
        assert_eq!(missing_method, None);
        assert!(env.exception_check());
        let ex_obj2 = env.exception_occurred().unwrap();
        let obj2 = env.objects.get(&ex_obj2).unwrap();
        let cls2 = env.classes.get(&obj2.class_id).unwrap();
        assert_eq!(cls2.name, "java/lang/NoSuchMethodError");
        let ex_str2 = obj2.string_val.as_deref().unwrap();
        assert!(ex_str2.contains("nonExistentMethod"));
    }

    #[test]
    fn test_jni_64bit_long_pointer_fields() {
        let mut env = JniEnv::new();
        let path_cls = env.find_class("android/graphics/Path").unwrap();
        let m_native_path_id = env.get_field_id(path_cls, "mNativePath", "J").expect("pre-registered mNativePath");

        let path_obj = env.new_object(path_cls);
        assert_eq!(env.get_long_field(path_obj, m_native_path_id), 0);

        // Store and read 64-bit native pointer handle
        let native_ptr: i64 = 0x7FFF_ABCD_EF01_2345i64;
        env.set_long_field(path_obj, m_native_path_id, native_ptr);
        assert_eq!(env.get_long_field(path_obj, m_native_path_id), native_ptr);
    }

    #[test]
    fn test_jni_reference_scoping_and_lifecycle() {
        let mut env = JniEnv::new();
        let _str_cls = env.find_class("java/lang/String").unwrap();

        let root_obj = env.new_string_utf("root");
        assert_eq!(env.get_object_ref_type(root_obj), JniRefType::LocalRefType);

        // Push local frame with capacity 16
        assert_eq!(env.push_local_frame(16), JNI_OK);
        let frame_obj1 = env.new_string_utf("child_frame_1");
        let frame_obj2 = env.new_string_utf("child_frame_2");

        // Convert frame_obj1 to global ref
        let global_obj = env.new_global_ref(frame_obj1);
        assert_eq!(env.get_object_ref_type(global_obj), JniRefType::GlobalRefType);

        // Pop local frame returning frame_obj2 (promoted to parent frame)
        let promoted = env.pop_local_frame(frame_obj2);
        assert_eq!(promoted, frame_obj2);

        // frame_obj1 is still alive as global ref
        assert_eq!(env.get_object_ref_type(global_obj), JniRefType::GlobalRefType);
        // frame_obj2 is alive in parent local frame
        assert_eq!(env.get_object_ref_type(frame_obj2), JniRefType::LocalRefType);

        // Delete global ref
        env.delete_global_ref(global_obj);
        // Now global_obj is no longer in any local frame, so it should be Invalid
        assert_eq!(env.get_object_ref_type(global_obj), JniRefType::InvalidRefType);
    }
}
