use super::*;
use tempfile::TempDir;

fn auth() -> Value {
    json!({"httpUrl":"http://127.0.0.1:4810", "token":"secret", "deviceId":"device", "scope":"account-a", "fileLimit":10485760, "imageLimit":10485760, "maxCount":100})
}
fn run(command: &str, directory: &TempDir, extra: Value) -> Value {
    let mut input = extra;
    input["command"] = command.into();
    input["directory"] = directory.path().to_str().unwrap().into();
    serde_json::from_str(&call_json(&input.to_string())).unwrap()
}
fn ok(command: &str, directory: &TempDir, extra: Value) -> Value {
    let result = run(command, directory, extra);
    assert_eq!(result["ok"], true, "{result}");
    result["value"].clone()
}
fn file_share(directory: &TempDir, bytes: &[u8]) -> Value {
    let path = directory.path().join("file.bin");
    fs::write(&path, bytes).unwrap();
    json!({"text":null,"html":null,"items":[{"path":path,"name":"file.bin","mimeType":"application/octet-stream"}]})
}
fn response(directory: &TempDir, status: u64, body: Value) -> Value {
    ok(
        "advance",
        directory,
        json!({"status":status,"body":body.to_string()}),
    )
}

#[test]
fn resumes_missing_chunks_and_publishes_only_after_content() {
    let directory = TempDir::new().unwrap();
    let bytes: Vec<_> = (0..(CHUNK_SIZE * 2 + 23))
        .map(|index| (index % 251) as u8)
        .collect();
    let plan = ok(
        "prepare",
        &directory,
        json!({"session":auth(),"share":file_share(&directory,&bytes)}),
    );
    let id = plan["blobs"][0]["fileId"].as_str().unwrap();
    let begin = ok("next", &directory, json!({"session":auth()}));
    assert!(begin["url"].as_str().unwrap().ends_with("/upload/begin"));
    response(
        &directory,
        200,
        json!({"status":"ready","missingChunks":STANDARD.encode([5]),"receivedBytes":CHUNK_SIZE}),
    );
    let first = ok("next", &directory, json!({"session":auth()}));
    assert!(first["url"].as_str().unwrap().ends_with("?index=0"));
    assert_eq!(
        fs::read(first["bodyPath"].as_str().unwrap()).unwrap(),
        &bytes[..CHUNK_SIZE as usize]
    );
    response(
        &directory,
        200,
        json!({"status":"accepted","missingChunks":STANDARD.encode([4]),"receivedBytes":CHUNK_SIZE*2}),
    );
    // Every call reloads its plan from disk, just as a newly launched process does.
    let last = ok("next", &directory, json!({"session":auth()}));
    assert!(last["url"].as_str().unwrap().ends_with("?index=2"));
    assert_eq!(
        fs::read(last["bodyPath"].as_str().unwrap()).unwrap(),
        &bytes[(CHUNK_SIZE * 2) as usize..]
    );
    response(&directory, 200, json!({"status":"stored","fileId":id}));
    let publish = ok("next", &directory, json!({"session":auth()}));
    assert!(publish["url"].as_str().unwrap().ends_with("/entries"));
    let body: Value =
        serde_json::from_slice(&fs::read(publish["bodyPath"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(body["entry"]["fileInfo"]["file.bin"]["f"], id);
    response(&directory, 200, json!({"entry":{"id":"server-id"}}));
    assert_eq!(
        ok("next", &directory, json!({"session":auth()}))["complete"],
        true
    );
    assert_eq!(read(directory.path()).unwrap().status, "complete");
}

#[test]
fn lost_publish_response_replays_identical_payload_and_keeps_credentials_out_of_disk() {
    let directory = TempDir::new().unwrap();
    ok(
        "prepare",
        &directory,
        json!({"session":auth(),"share":{"text":"hello","items":[]}}),
    );
    let request = ok("next", &directory, json!({"session":auth()}));
    let original = fs::read(request["bodyPath"].as_str().unwrap()).unwrap();
    assert_eq!(
        run("advance", &directory, json!({"status":401,"body":"{}"}))["ok"],
        false
    );
    ok("fail", &directory, json!({"error":"network disconnected"}));
    let request = ok("next", &directory, json!({"session":auth()}));
    assert_eq!(
        original,
        fs::read(request["bodyPath"].as_str().unwrap()).unwrap()
    );
    for name in ["upload.json", "request.body"] {
        assert!(!fs::read_to_string(directory.path().join(name))
            .unwrap()
            .contains("secret"));
    }
    assert_eq!(read(directory.path()).unwrap().entry_index, 0);
}

#[test]
fn account_switch_cannot_retarget_a_staged_share() {
    let directory = TempDir::new().unwrap();
    ok(
        "prepare",
        &directory,
        json!({"session":auth(),"share":{"text":"private","items":[]}}),
    );
    let mut other = auth();
    other["scope"] = "account-b".into();
    let result = run("next", &directory, json!({"session":other}));
    assert_eq!(result["ok"], false);
    assert!(!directory.path().join("request.body").exists());
}

#[test]
fn retired_chunk_ledger_is_restarted_at_most_three_times() {
    let directory = TempDir::new().unwrap();
    ok(
        "prepare",
        &directory,
        json!({"session":auth(),"share":file_share(&directory,b"data")}),
    );
    for index in 0..4 {
        ok("next", &directory, json!({"session":auth()}));
        response(
            &directory,
            200,
            json!({"status":"ready","missingChunks":STANDARD.encode([1]),"receivedBytes":0}),
        );
        ok("next", &directory, json!({"session":auth()}));
        let result = run("advance", &directory, json!({"status":404,"body":"{}"}));
        assert_eq!(result["ok"], index < 3);
    }
}

#[test]
fn malformed_or_nonprogressing_ledgers_do_not_advance_the_plan() {
    let directory = TempDir::new().unwrap();
    ok(
        "prepare",
        &directory,
        json!({"session":auth(),"share":file_share(&directory,b"data")}),
    );
    ok("next", &directory, json!({"session":auth()}));
    assert_eq!(
        run(
            "advance",
            &directory,
            json!({"status":200,"body":json!({"status":"ready","missingChunks":"","receivedBytes":0}).to_string()})
        )["ok"],
        false
    );
    response(
        &directory,
        200,
        json!({"status":"ready","missingChunks":STANDARD.encode([1]),"receivedBytes":0}),
    );
    ok("next", &directory, json!({"session":auth()}));
    assert_eq!(
        run(
            "advance",
            &directory,
            json!({"status":200,"body":json!({"status":"accepted","missingChunks":STANDARD.encode([1]),"receivedBytes":0}).to_string()})
        )["ok"],
        false
    );
    assert_eq!(read(directory.path()).unwrap().blob_index, 0);
}

#[test]
fn over_limit_preparation_leaves_original_available_for_main_app_import() {
    let directory = TempDir::new().unwrap();
    let share = file_share(&directory, b"content");
    let mut limited = auth();
    limited["fileLimit"] = 1.into();
    assert_eq!(
        run(
            "prepare",
            &directory,
            json!({"session":limited,"share":share})
        )["ok"],
        false
    );
    assert!(!directory.path().join("upload.json").exists());
    assert_eq!(
        fs::read(directory.path().join("file.bin")).unwrap(),
        b"content"
    );
}

#[test]
fn screenshot_is_an_ordinary_file_without_transcoding() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("picture.png");
    let original = b"original screenshot file bytes";
    fs::write(&path, original).unwrap();
    let plan = ok(
        "prepare",
        &directory,
        json!({"session":auth(),"share":{"items":[{"path":path,"name":"picture.png","mimeType":"image/png"}]}}),
    );
    assert_eq!(plan["entries"][0]["kind"], "files");
    assert_eq!(plan["blobs"][0]["kind"], "files");
    let full = fs::read(plan["blobs"][0]["path"].as_str().unwrap()).unwrap();
    assert_eq!(full, original);
    assert_eq!(
        plan["entries"][0]["fileInfo"]["picture.png"]["f"],
        format!("{:x}", Sha256::digest(full))
    );
}

#[test]
fn malformed_ffi_input_returns_an_error_and_allocations_can_be_released() {
    let input = CString::new("not json").unwrap();
    unsafe {
        let output = cliproam_share_call(input.as_ptr());
        let value: Value = serde_json::from_str(CStr::from_ptr(output).to_str().unwrap()).unwrap();
        assert_eq!(value["ok"], false);
        cliproam_share_free(output);
        let output = cliproam_share_call(std::ptr::null());
        cliproam_share_free(output);
    }
}

#[test]
fn duplicate_file_names_keep_both_payloads_and_unicode_names_fit_server_limits() {
    let directory = TempDir::new().unwrap();
    let first = directory.path().join("one");
    let second = directory.path().join("two");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    let name = "😀".repeat(200);
    let plan = ok(
        "prepare",
        &directory,
        json!({"session":auth(),"share":{"items":[
            {"path":first,"name":name},{"path":second,"name":name}
        ]}}),
    );
    let tree = plan["entries"][0]["fileInfo"].as_object().unwrap();
    assert_eq!(tree.len(), 2);
    for name in tree.keys() {
        assert!(name.encode_utf16().count() <= 255);
    }
    let ids: std::collections::HashSet<_> = tree
        .values()
        .map(|value| value["f"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
}
