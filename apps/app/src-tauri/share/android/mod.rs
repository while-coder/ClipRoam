//! The only Android boundary: forward JSON between Kotlin and the Rust core.
use ::jni::{
    objects::{JObject, JString},
    sys::jstring,
    JNIEnv,
};

#[no_mangle]
pub extern "system" fn Java_com_whilecode_cliproam_share_ShareNative_call(
    mut env: JNIEnv,
    _receiver: JObject,
    input: JString,
) -> jstring {
    let output = match env.get_string(&input) {
        Ok(input) => cliproam_share_upload::call_json(&String::from(input)),
        Err(error) => serde_json::json!({"ok":false,"error":error.to_string()}).to_string(),
    };
    env.new_string(output)
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
