use ::jni::{
    objects::{JObject, JString},
    sys::jstring,
    JNIEnv,
};

#[no_mangle]
pub extern "system" fn Java_com_whilecode_cliproam_share_ShareNative_call(
    mut env: JNIEnv,
    _receiver: JObject,
    context: JObject,
    input: JString,
) -> jstring {
    let result = (|| {
        let input: String = env.get_string(&input).map_err(|e| e.to_string())?.into();
        let input = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        super::dispatch(&mut env, &context, input)
    })();
    let output = match result {
        Ok(value) => serde_json::json!({"ok":true,"value":value}).to_string(),
        Err(error) => {
            let _ = env.exception_clear();
            serde_json::json!({"ok":false,"error":error}).to_string()
        }
    };
    env.new_string(output)
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
