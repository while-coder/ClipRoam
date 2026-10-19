//! Android system adapter. Kotlin only forwards Activity/Worker lifecycle calls.
mod jni;

use ::jni::{
    objects::{JByteArray, JObject, JString, JValue},
    JNIEnv,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::OnceLock,
};
use tauri::Emitter;
type Result<T> = std::result::Result<T, String>;
static APP: OnceLock<tauri::AppHandle> = OnceLock::new();
pub fn attach(app: tauri::AppHandle) {
    let _ = APP.set(app);
}
fn notify() {
    if let Some(app) = APP.get() {
        let _ = app.emit("cliproam://share-received", json!({}));
    }
}
fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
fn obj<'a>(
    env: &mut JNIEnv<'a>,
    target: &JObject<'_>,
    name: &str,
    sig: &str,
    args: &[JValue],
) -> Result<JObject<'a>> {
    env.call_method(target, name, sig, args)
        .and_then(|v| v.l())
        .map_err(err)
}
fn text(env: &mut JNIEnv, value: JObject) -> Result<String> {
    if value.is_null() {
        return Ok(String::new());
    }
    env.get_string(&JString::from(value))
        .map(|v| v.into())
        .map_err(err)
}
fn string<'a>(env: &mut JNIEnv<'a>, value: &str) -> Result<JObject<'a>> {
    env.new_string(value).map(JObject::from).map_err(err)
}
fn files(env: &mut JNIEnv, ctx: &JObject) -> Result<PathBuf> {
    let file = obj(env, ctx, "getFilesDir", "()Ljava/io/File;", &[])?;
    let path = obj(env, &file, "getAbsolutePath", "()Ljava/lang/String;", &[])?;
    Ok(text(env, path)?.into())
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes).map_err(err)?;
    fs::rename(temp, path).map_err(err)
}
fn core(command: &str, directory: &Path, mut input: Value) -> Result<Value> {
    input["command"] = command.into();
    input["directory"] = directory.to_string_lossy().as_ref().into();
    let output: Value =
        serde_json::from_str(&cliproam_share_upload::call_json(&input.to_string())).map_err(err)?;
    if output["ok"] == true {
        Ok(output["value"].clone())
    } else {
        Err(output["error"].as_str().unwrap_or("上传失败").into())
    }
}
fn key<'a>(env: &mut JNIEnv<'a>) -> Result<JObject<'a>> {
    let provider = string(env, "AndroidKeyStore")?;
    let alias = string(env, "cliproam-share-auth")?;
    let store = env
        .call_static_method(
            "java/security/KeyStore",
            "getInstance",
            "(Ljava/lang/String;)Ljava/security/KeyStore;",
            &[JValue::Object(&provider)],
        )
        .and_then(|v| v.l())
        .map_err(err)?;
    env.call_method(
        &store,
        "load",
        "(Ljava/io/InputStream;[C)V",
        &[
            JValue::Object(&JObject::null()),
            JValue::Object(&JObject::null()),
        ],
    )
    .map_err(err)?;
    let found = obj(
        env,
        &store,
        "getKey",
        "(Ljava/lang/String;[C)Ljava/security/Key;",
        &[JValue::Object(&alias), JValue::Object(&JObject::null())],
    )?;
    if !found.is_null() {
        return Ok(found);
    }
    let builder = env
        .new_object(
            "android/security/keystore/KeyGenParameterSpec$Builder",
            "(Ljava/lang/String;I)V",
            &[JValue::Object(&alias), JValue::Int(3)],
        )
        .map_err(err)?;
    for (method, value) in [
        ("setBlockModes", "GCM"),
        ("setEncryptionPaddings", "NoPadding"),
    ] {
        let value = string(env, value)?;
        let array = env
            .new_object_array(1, "java/lang/String", &value)
            .map_err(err)?;
        env.call_method(
            &builder,
            method,
            "([Ljava/lang/String;)Landroid/security/keystore/KeyGenParameterSpec$Builder;",
            &[JValue::Object(&array)],
        )
        .map_err(err)?;
    }
    let spec = obj(
        env,
        &builder,
        "build",
        "()Landroid/security/keystore/KeyGenParameterSpec;",
        &[],
    )?;
    let aes = string(env, "AES")?;
    let generator = env
        .call_static_method(
            "javax/crypto/KeyGenerator",
            "getInstance",
            "(Ljava/lang/String;Ljava/lang/String;)Ljavax/crypto/KeyGenerator;",
            &[JValue::Object(&aes), JValue::Object(&provider)],
        )
        .and_then(|v| v.l())
        .map_err(err)?;
    env.call_method(
        &generator,
        "init",
        "(Ljava/security/spec/AlgorithmParameterSpec;)V",
        &[JValue::Object(&spec)],
    )
    .map_err(err)?;
    obj(
        env,
        &generator,
        "generateKey",
        "()Ljavax/crypto/SecretKey;",
        &[],
    )
}
fn credentials(
    env: &mut JNIEnv,
    root: &Path,
    id: &str,
    value: Option<Option<&Value>>,
) -> Result<Option<Value>> {
    if id != "current" && uuid::Uuid::parse_str(id).is_err() {
        return Err("分享标识无效".into());
    }
    let directory = root.join("share-auth");
    fs::create_dir_all(&directory).map_err(err)?;
    let path = directory.join(id);
    if matches!(value, Some(None)) {
        let _ = fs::remove_file(path);
        return Ok(None);
    }
    let input = if let Some(Some(v)) = value {
        serde_json::to_vec(v).map_err(err)?
    } else if path.exists() {
        fs::read(&path).map_err(err)?
    } else {
        return Ok(None);
    };
    let algorithm = string(env, "AES/GCM/NoPadding")?;
    let cipher = env
        .call_static_method(
            "javax/crypto/Cipher",
            "getInstance",
            "(Ljava/lang/String;)Ljavax/crypto/Cipher;",
            &[JValue::Object(&algorithm)],
        )
        .and_then(|v| v.l())
        .map_err(err)?;
    let key = key(env)?;
    let encrypt = value.is_some();
    let data = if encrypt {
        env.call_method(
            &cipher,
            "init",
            "(ILjava/security/Key;)V",
            &[JValue::Int(1), JValue::Object(&key)],
        )
        .map_err(err)?;
        input
    } else {
        if input.len() < 12 {
            return Err("分享登录状态损坏".into());
        }
        let iv = env.byte_array_from_slice(&input[..12]).map_err(err)?;
        let spec = env
            .new_object(
                "javax/crypto/spec/GCMParameterSpec",
                "(I[B)V",
                &[JValue::Int(128), JValue::Object(&iv)],
            )
            .map_err(err)?;
        env.call_method(
            &cipher,
            "init",
            "(ILjava/security/Key;Ljava/security/spec/AlgorithmParameterSpec;)V",
            &[JValue::Int(2), JValue::Object(&key), JValue::Object(&spec)],
        )
        .map_err(err)?;
        input[12..].to_vec()
    };
    let bytes = env.byte_array_from_slice(&data).map_err(err)?;
    let output = obj(env, &cipher, "doFinal", "([B)[B", &[JValue::Object(&bytes)])?;
    let output = env
        .convert_byte_array(JByteArray::from(output))
        .map_err(err)?;
    if encrypt {
        let iv = obj(env, &cipher, "getIV", "()[B", &[])?;
        let mut encrypted = env.convert_byte_array(JByteArray::from(iv)).map_err(err)?;
        encrypted.extend(output);
        atomic(&path, &encrypted)?;
        Ok(None)
    } else {
        serde_json::from_slice(&output).map(Some).map_err(err)
    }
}
fn schedule(env: &mut JNIEnv, ctx: &JObject, id: &str) -> Result<()> {
    let id_value = string(env, id)?;
    let id_key = string(env, "id")?;
    let data = env
        .new_object("androidx/work/Data$Builder", "()V", &[])
        .map_err(err)?;
    env.call_method(
        &data,
        "putString",
        "(Ljava/lang/String;Ljava/lang/String;)Landroidx/work/Data$Builder;",
        &[JValue::Object(&id_key), JValue::Object(&id_value)],
    )
    .map_err(err)?;
    let data = obj(env, &data, "build", "()Landroidx/work/Data;", &[])?;
    let class = env
        .find_class("com/whilecode/cliproam/share/ShareUploadWorker")
        .map_err(err)?;
    let builder = env
        .new_object(
            "androidx/work/OneTimeWorkRequest$Builder",
            "(Ljava/lang/Class;)V",
            &[JValue::Object(&class)],
        )
        .map_err(err)?;
    env.call_method(
        &builder,
        "setInputData",
        "(Landroidx/work/Data;)Landroidx/work/WorkRequest$Builder;",
        &[JValue::Object(&data)],
    )
    .map_err(err)?;
    let constraints = env
        .new_object("androidx/work/Constraints$Builder", "()V", &[])
        .map_err(err)?;
    let connected = env
        .get_static_field(
            "androidx/work/NetworkType",
            "CONNECTED",
            "Landroidx/work/NetworkType;",
        )
        .and_then(|v| v.l())
        .map_err(err)?;
    env.call_method(
        &constraints,
        "setRequiredNetworkType",
        "(Landroidx/work/NetworkType;)Landroidx/work/Constraints$Builder;",
        &[JValue::Object(&connected)],
    )
    .map_err(err)?;
    let constraints = obj(
        env,
        &constraints,
        "build",
        "()Landroidx/work/Constraints;",
        &[],
    )?;
    env.call_method(
        &builder,
        "setConstraints",
        "(Landroidx/work/Constraints;)Landroidx/work/WorkRequest$Builder;",
        &[JValue::Object(&constraints)],
    )
    .map_err(err)?;
    let sdk = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|v| v.i())
        .map_err(err)?;
    if sdk >= 31 {
        let fallback = env
            .get_static_field(
                "androidx/work/OutOfQuotaPolicy",
                "RUN_AS_NON_EXPEDITED_WORK_REQUEST",
                "Landroidx/work/OutOfQuotaPolicy;",
            )
            .and_then(|v| v.l())
            .map_err(err)?;
        env.call_method(
            &builder,
            "setExpedited",
            "(Landroidx/work/OutOfQuotaPolicy;)Landroidx/work/WorkRequest$Builder;",
            &[JValue::Object(&fallback)],
        )
        .map_err(err)?;
    }
    let request = obj(env, &builder, "build", "()Landroidx/work/WorkRequest;", &[])?;
    let manager = env
        .call_static_method(
            "androidx/work/WorkManager",
            "getInstance",
            "(Landroid/content/Context;)Landroidx/work/WorkManager;",
            &[JValue::Object(ctx)],
        )
        .and_then(|v| v.l())
        .map_err(err)?;
    let name = string(env, &format!("cliproam-share-{id}"))?;
    let keep = env
        .get_static_field(
            "androidx/work/ExistingWorkPolicy",
            "KEEP",
            "Landroidx/work/ExistingWorkPolicy;",
        )
        .and_then(|v| v.l())
        .map_err(err)?;
    env.call_method(&manager,"enqueueUniqueWork","(Ljava/lang/String;Landroidx/work/ExistingWorkPolicy;Landroidx/work/OneTimeWorkRequest;)Landroidx/work/Operation;",&[JValue::Object(&name),JValue::Object(&keep),JValue::Object(&request)]).map_err(err)?;
    Ok(())
}
fn resume(env: &mut JNIEnv, ctx: &JObject, root: &Path, auth: &Value) -> Result<()> {
    let inbox = root.join("cliproam-shares");
    if !inbox.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(inbox).map_err(err)? {
        let directory = entry.map_err(err)?.path();
        let id = directory
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let plan: Option<Value> = fs::read(directory.join("upload.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let manifest: Option<Value> = fs::read(directory.join("manifest.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let scope = plan
            .as_ref()
            .map(|v| &v["scope"])
            .or_else(|| manifest.as_ref().map(|v| &v["ownerScope"]));
        if scope != Some(&auth["scope"]) {
            continue;
        }
        if plan.as_ref().is_some_and(|p| p["status"] == "complete") {
            fs::remove_dir_all(&directory).map_err(err)?;
            credentials(env, root, &id, Some(None))?;
        } else if plan.is_some() || directory.join("native").exists() {
            credentials(env, root, &id, Some(Some(auth)))?;
            schedule(env, ctx, &id)?;
        }
    }
    Ok(())
}
fn intent_text(
    env: &mut JNIEnv,
    intent: &JObject,
    key: &str,
    method: &str,
    sig: &str,
) -> Result<String> {
    let key = string(env, key)?;
    let value = obj(env, intent, method, sig, &[JValue::Object(&key)])?;
    if value.is_null() {
        return Ok(String::new());
    }
    let value = obj(env, &value, "toString", "()Ljava/lang/String;", &[])?;
    text(env, value)
}
fn stage(env: &mut JNIEnv, ctx: &JObject, root: &Path) -> Result<Value> {
    let intent = obj(env, ctx, "getIntent", "()Landroid/content/Intent;", &[])?;
    let text_value = intent_text(
        env,
        &intent,
        "android.intent.extra.TEXT",
        "getCharSequenceExtra",
        "(Ljava/lang/String;)Ljava/lang/CharSequence;",
    )?;
    let html = intent_text(
        env,
        &intent,
        "android.intent.extra.HTML_TEXT",
        "getStringExtra",
        "(Ljava/lang/String;)Ljava/lang/String;",
    )?;
    let key = string(env, "android.intent.extra.STREAM")?;
    let mut uris = Vec::new();
    let single = obj(
        env,
        &intent,
        "getParcelableExtra",
        "(Ljava/lang/String;)Landroid/os/Parcelable;",
        &[JValue::Object(&key)],
    )?;
    if !single.is_null() {
        uris.push(single)
    }
    let multiple = obj(
        env,
        &intent,
        "getParcelableArrayListExtra",
        "(Ljava/lang/String;)Ljava/util/ArrayList;",
        &[JValue::Object(&key)],
    )?;
    if !multiple.is_null() {
        let count = env
            .call_method(&multiple, "size", "()I", &[])
            .and_then(|v| v.i())
            .map_err(err)?;
        for i in 0..count {
            uris.push(obj(
                env,
                &multiple,
                "get",
                "(I)Ljava/lang/Object;",
                &[JValue::Int(i)],
            )?);
        }
    }
    let clip = obj(
        env,
        &intent,
        "getClipData",
        "()Landroid/content/ClipData;",
        &[],
    )?;
    if !clip.is_null() {
        let count = env
            .call_method(&clip, "getItemCount", "()I", &[])
            .and_then(|v| v.i())
            .map_err(err)?;
        for i in 0..count {
            let item = obj(
                env,
                &clip,
                "getItemAt",
                "(I)Landroid/content/ClipData$Item;",
                &[JValue::Int(i)],
            )?;
            let uri = obj(env, &item, "getUri", "()Landroid/net/Uri;", &[])?;
            if !uri.is_null() {
                uris.push(uri)
            }
        }
    }
    if text_value.is_empty() && uris.is_empty() {
        return Err("没有可接收的分享内容".into());
    }
    let auth = credentials(env, root, "current", None).unwrap_or_else(|_| {
        let _ = env.exception_clear();
        None
    });
    let id = uuid::Uuid::new_v4().to_string();
    let directory = root.join("cliproam-shares").join(&id);
    fs::create_dir_all(directory.join("files")).map_err(err)?;
    let resolver = obj(
        env,
        ctx,
        "getContentResolver",
        "()Landroid/content/ContentResolver;",
        &[],
    )?;
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for uri in uris {
        let value = obj(env, &uri, "toString", "()Ljava/lang/String;", &[])?;
        if !seen.insert(text(env, value)?) {
            continue;
        }
        let column = string(env, "_display_name")?;
        let projection = env
            .new_object_array(1, "java/lang/String", &column)
            .map_err(err)?;
        let cursor=obj(env,&resolver,"query","(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",&[JValue::Object(&uri),JValue::Object(&projection),JValue::Object(&JObject::null()),JValue::Object(&JObject::null()),JValue::Object(&JObject::null())])?;
        let mut name = format!("shared-{}", items.len() + 1);
        if !cursor.is_null() {
            if env
                .call_method(&cursor, "moveToFirst", "()Z", &[])
                .and_then(|v| v.z())
                .map_err(err)?
            {
                let value = obj(
                    env,
                    &cursor,
                    "getString",
                    "(I)Ljava/lang/String;",
                    &[JValue::Int(0)],
                )?;
                let value = text(env, value)?;
                if !value.is_empty() {
                    name = value
                }
            }
            env.call_method(&cursor, "close", "()V", &[]).map_err(err)?;
        }
        name = name.replace(['/', '\\'], "_");
        let target = directory.join("files").join(items.len().to_string());
        fs::create_dir_all(&target).map_err(err)?;
        let target = target.join(&name);
        let input = obj(
            env,
            &resolver,
            "openInputStream",
            "(Landroid/net/Uri;)Ljava/io/InputStream;",
            &[JValue::Object(&uri)],
        )?;
        if input.is_null() {
            return Err("无法读取分享文件".into());
        }
        let mut output = fs::File::create(&target).map_err(err)?;
        let buffer = env.new_byte_array(128 * 1024).map_err(err)?;
        let copied = (|| -> Result<()> {
            loop {
                let count = env
                    .call_method(&input, "read", "([B)I", &[JValue::Object(&buffer)])
                    .and_then(|v| v.i())
                    .map_err(err)?;
                if count < 0 {
                    break;
                }
                let bytes = env.convert_byte_array(&buffer).map_err(err)?;
                output.write_all(&bytes[..count as usize]).map_err(err)?;
            }
            Ok(())
        })();
        let _ = env.call_method(&input, "close", "()V", &[]);
        copied?;
        items.push(json!({"path":target,"name":name,"mimeType":"application/octet-stream"}));
    }
    if let Some(auth) = &auth {
        credentials(env, root, &id, Some(Some(auth)))?;
        fs::write(directory.join("native"), []).map_err(err)?;
    }
    atomic(&directory.join("manifest.json"),&serde_json::to_vec(&json!({"id":id,"text":text_value,"html":if html.is_empty(){Value::Null}else{html.into()},"items":items,"ownerScope":auth.as_ref().map(|a|&a["scope"])})).map_err(err)?)?;
    let uploading = auth.is_some() && schedule(env, ctx, &id).is_ok();
    if !uploading {
        let _ = fs::remove_file(directory.join("native"));
        credentials(env, root, &id, Some(None))?;
    }
    notify();
    Ok(
        json!({"message":if uploading{"已加入后台上传，无需打开 ClipRoam。"}else{"内容已保存，请打开 ClipRoam 登录后上传。"}}),
    )
}
fn execute(env: &mut JNIEnv, request: &Value, token: &str) -> Result<(u16, String)> {
    env.push_local_frame(64).map_err(err)?;
    let result = execute_inner(env, request, token);
    if result.is_err() {
        let _ = env.exception_clear();
    }
    unsafe {
        env.pop_local_frame(&JObject::null()).map_err(err)?;
    }
    result
}
fn execute_inner(env: &mut JNIEnv, request: &Value, token: &str) -> Result<(u16, String)> {
    let address = string(env, request["url"].as_str().ok_or("请求地址缺失")?)?;
    let url = env
        .new_object(
            "java/net/URL",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&address)],
        )
        .map_err(err)?;
    let connection = obj(
        env,
        &url,
        "openConnection",
        "()Ljava/net/URLConnection;",
        &[],
    )?;
    let result = (|| -> Result<(u16, String)> {
        for (method, value) in [("setConnectTimeout", 30000), ("setReadTimeout", 120000)] {
            env.call_method(&connection, method, "(I)V", &[JValue::Int(value)])
                .map_err(err)?;
        }
        env.call_method(
            &connection,
            "setInstanceFollowRedirects",
            "(Z)V",
            &[JValue::Bool(0)],
        )
        .map_err(err)?;
        env.call_method(&connection, "setDoOutput", "(Z)V", &[JValue::Bool(1)])
            .map_err(err)?;
        let method = string(env, request["method"].as_str().unwrap_or("POST"))?;
        env.call_method(
            &connection,
            "setRequestMethod",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&method)],
        )
        .map_err(err)?;
        for (key, value) in [
            ("Authorization", format!("Bearer {token}")),
            (
                "Content-Type",
                request["contentType"]
                    .as_str()
                    .unwrap_or("application/json")
                    .to_owned(),
            ),
        ] {
            let key = string(env, key)?;
            let value = string(env, &value)?;
            env.call_method(
                &connection,
                "setRequestProperty",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&key), JValue::Object(&value)],
            )
            .map_err(err)?;
        }
        let mut body =
            fs::File::open(request["bodyPath"].as_str().ok_or("请求文件缺失")?).map_err(err)?;
        let size = body.metadata().map_err(err)?.len();
        env.call_method(
            &connection,
            "setFixedLengthStreamingMode",
            "(J)V",
            &[JValue::Long(size as i64)],
        )
        .map_err(err)?;
        let output = obj(
            env,
            &connection,
            "getOutputStream",
            "()Ljava/io/OutputStream;",
            &[],
        )?;
        let mut bytes = [0u8; 128 * 1024];
        loop {
            let count = body.read(&mut bytes).map_err(err)?;
            if count == 0 {
                break;
            }
            let chunk = env.byte_array_from_slice(&bytes[..count]).map_err(err)?;
            env.call_method(&output, "write", "([B)V", &[JValue::Object(&chunk)])
                .map_err(err)?;
            env.delete_local_ref(chunk).map_err(err)?;
        }
        env.call_method(&output, "close", "()V", &[]).map_err(err)?;
        let status = env
            .call_method(&connection, "getResponseCode", "()I", &[])
            .and_then(|v| v.i())
            .map_err(err)? as u16;
        let method = if (200..300).contains(&status) {
            "getInputStream"
        } else {
            "getErrorStream"
        };
        let input = obj(env, &connection, method, "()Ljava/io/InputStream;", &[])?;
        let mut response = Vec::new();
        if !input.is_null() {
            let buffer = env.new_byte_array(8192).map_err(err)?;
            loop {
                let count = env
                    .call_method(&input, "read", "([B)I", &[JValue::Object(&buffer)])
                    .and_then(|v| v.i())
                    .map_err(err)?;
                if count < 0 {
                    break;
                }
                let bytes = env.convert_byte_array(&buffer).map_err(err)?;
                response.extend_from_slice(&bytes[..count as usize]);
                if response.len() > 1024 * 1024 {
                    return Err("服务器响应过大".into());
                }
            }
            env.call_method(&input, "close", "()V", &[]).map_err(err)?;
        }
        Ok((status, String::from_utf8_lossy(&response).into_owned()))
    })();
    if result.is_err() {
        let _ = env.exception_clear();
    }
    let _ = env.call_method(&connection, "disconnect", "()V", &[]);
    result
}
fn upload(env: &mut JNIEnv, root: &Path, id: &str, attempt: u64) -> Result<Value> {
    let directory = root.join("cliproam-shares").join(id);
    if !directory.exists() {
        return Ok(json!({"status":"success"}));
    }
    let mut retry = false;
    let result = (|| -> Result<()> {
        let auth = credentials(env, root, id, None)?.ok_or("请打开 ClipRoam 登录")?;
        if !directory.join("upload.json").exists() {
            let share: Value =
                serde_json::from_slice(&fs::read(directory.join("manifest.json")).map_err(err)?)
                    .map_err(err)?;
            core("prepare", &directory, json!({"session":auth,"share":share}))?;
        }
        loop {
            let request = core("next", &directory, json!({"session":auth}))?;
            if request["complete"] == true {
                credentials(env, root, id, Some(None))?;
                break;
            }
            let (status, body) = match execute(env, &request, auth["token"].as_str().unwrap_or(""))
            {
                Ok(v) => v,
                Err(e) => {
                    retry = true;
                    return Err(e);
                }
            };
            retry = status >= 500;
            core("advance", &directory, json!({"status":status,"body":body}))?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        if directory.join("upload.json").exists() {
            let _ = core("fail", &directory, json!({"error":error}));
        } else {
            let _ = fs::remove_file(directory.join("native"));
            credentials(env, root, id, Some(None))?;
        }
        notify();
        Ok(json!({"status":if retry&&attempt<3{"retry"}else{"failed"}}))
    } else {
        notify();
        Ok(json!({"status":"success"}))
    }
}
pub fn dispatch(env: &mut JNIEnv, ctx: &JObject, input: Value) -> Result<Value> {
    let root = files(env, ctx)?;
    match input["command"].as_str().unwrap_or("") {
        "stage" => stage(env, ctx, &root),
        "upload" => {
            let id = input["id"].as_str().ok_or("分享标识缺失")?;
            uuid::Uuid::parse_str(id).map_err(err)?;
            upload(env, &root, id, input["attempt"].as_u64().unwrap_or(0))
        }
        "configure" => {
            let auth = (!input["session"].is_null()).then_some(&input["session"]);
            credentials(env, &root, "current", Some(auth))?;
            if let Some(auth) = auth {
                resume(env, ctx, &root, auth)?;
            }
            Ok(json!({}))
        }
        "resume" => {
            if let Some(auth) = credentials(env, &root, "current", None)? {
                resume(env, ctx, &root, &auth)?;
            }
            notify();
            Ok(json!({}))
        }
        "pending" => {
            let mut shares = Vec::new();
            let inbox = root.join("cliproam-shares");
            if inbox.exists() {
                for entry in fs::read_dir(inbox).map_err(err)? {
                    let dir = entry.map_err(err)?.path();
                    if dir.join("upload.json").exists() || dir.join("native").exists() {
                        continue;
                    }
                    if let Ok(bytes) = fs::read(dir.join("manifest.json")) {
                        let share: Value = serde_json::from_slice(&bytes).map_err(err)?;
                        if share["ownerScope"].is_null() || share["ownerScope"] == input["scope"] {
                            shares.push(share)
                        }
                    }
                }
            }
            Ok(json!(shares))
        }
        "ack" => {
            let id = input["id"].as_str().ok_or("分享标识缺失")?;
            uuid::Uuid::parse_str(id).map_err(err)?;
            credentials(env, &root, id, Some(None))?;
            let directory = root.join("cliproam-shares").join(id);
            if directory.exists() {
                fs::remove_dir_all(directory).map_err(err)?;
            }
            Ok(json!({}))
        }
        _ => Err("未知 Android 分享操作".into()),
    }
}
