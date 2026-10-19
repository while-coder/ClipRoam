//! iOS lifecycle, Keychain, share providers and background URLSession adapter.
//! The Swift extension only forwards its view-controller entry point here.
use super::*;
use block2::{DynBlock, RcBlock};
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    data::CFData,
    dictionary::CFDictionary,
    string::CFString,
};
use objc2::{
    class, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, ProtocolObject, Sel},
    sel, ClassType,
};
use objc2_foundation::*;
use security_framework_sys::{
    access_control::kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly, item::*, keychain_item::*,
};
use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{Arc, Mutex, OnceLock},
};

const GROUP: &str = "group.com.whilecode.cliproam";
const PREFIX: &str = "com.whilecode.cliproam.share.upload.";
#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecAttrAccessible: core_foundation::string::CFStringRef;
}
static NOTIFY: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
pub fn set_notify(callback: Box<dyn Fn() + Send + Sync>) {
    let _ = NOTIFY.set(callback);
}
fn notify() {
    if let Some(callback) = NOTIFY.get() {
        callback()
    }
}
fn root() -> Result<PathBuf> {
    let url = NSFileManager::defaultManager()
        .containerURLForSecurityApplicationGroupIdentifier(&NSString::from_str(GROUP))
        .ok_or("无法访问 App Group，请检查签名权限")?;
    let path = url.path().ok_or("共享容器地址无效")?;
    let directory = PathBuf::from(path.to_string()).join("ShareInbox");
    fs::create_dir_all(&directory).map_err(io)?;
    Ok(directory)
}
fn directory(id: &str) -> Result<PathBuf> {
    if id.len() != 36 || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
        return Err("分享标识无效".into());
    }
    Ok(root()?.join(id))
}
unsafe fn cf(value: core_foundation::string::CFStringRef) -> CFType {
    CFString::wrap_under_get_rule(value).as_CFType()
}
fn credentials(id: &str, value: Option<Option<&Value>>) -> Result<Option<Value>> {
    unsafe {
        let group = NSBundle::mainBundle()
            .objectForInfoDictionaryKey(ns_string!("ShareKeychainAccessGroup"))
            .and_then(|v| v.downcast::<NSString>().ok())
            .ok_or("分享 Keychain 配置不可用")?
            .to_string();
        if group.contains("$(") {
            return Err("分享 Keychain 签名配置无效".into());
        }
        let mut pairs = vec![
            (cf(kSecClass), cf(kSecClassGenericPassword)),
            (
                cf(kSecAttrService),
                CFString::new("com.whilecode.cliproam.share").as_CFType(),
            ),
            (cf(kSecAttrAccessGroup), CFString::new(&group).as_CFType()),
            (cf(kSecAttrAccount), CFString::new(id).as_CFType()),
        ];
        let query = CFDictionary::from_CFType_pairs(&pairs);
        if let Some(value) = value {
            if let Some(value) = value {
                let data = CFData::from_buffer(&serde_json::to_vec(value).map_err(io)?);
                let attrs = CFDictionary::from_CFType_pairs(&[
                    (cf(kSecValueData), data.as_CFType()),
                    (
                        cf(kSecAttrAccessible),
                        cf(kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly),
                    ),
                ]);
                let mut status =
                    SecItemUpdate(query.as_concrete_TypeRef(), attrs.as_concrete_TypeRef());
                if status == -25300 {
                    pairs.extend([
                        (cf(kSecValueData), data.as_CFType()),
                        (
                            cf(kSecAttrAccessible),
                            cf(kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly),
                        ),
                    ]);
                    status = SecItemAdd(
                        CFDictionary::from_CFType_pairs(&pairs).as_concrete_TypeRef(),
                        std::ptr::null_mut(),
                    );
                }
                if status != 0 {
                    return Err(format!("无法保存分享登录状态（{status}）"));
                }
            } else {
                SecItemDelete(query.as_concrete_TypeRef());
            }
            return Ok(None);
        }
        pairs.push((cf(kSecReturnData), CFBoolean::true_value().as_CFType()));
        let query = CFDictionary::from_CFType_pairs(&pairs);
        let mut output = std::ptr::null();
        let status = SecItemCopyMatching(query.as_concrete_TypeRef(), &mut output);
        if status == -25300 {
            return Ok(None);
        }
        if status != 0 {
            return Err(format!("无法读取分享登录状态（{status}）"));
        }
        let data = CFData::wrap_under_create_rule(output.cast());
        serde_json::from_slice(data.bytes()).map(Some).map_err(io)
    }
}
#[derive(Default)]
struct State {
    sessions: HashMap<String, Retained<NSURLSession>>,
    responses: HashMap<String, Vec<u8>>,
    completions: HashMap<String, RcBlock<dyn Fn()>>,
    observer: Option<Retained<AnyObject>>,
}
// Access is serialized by the mutex; completion blocks are invoked only on the
// main operation queue. NSURLSession itself is thread-safe.
unsafe impl Send for State {}
static STATE: OnceLock<Mutex<State>> = OnceLock::new();
fn state() -> &'static Mutex<State> {
    STATE.get_or_init(Default::default)
}
fn on_main(callback: impl Fn() + 'static) {
    unsafe {
        NSOperationQueue::mainQueue().addOperationWithBlock(&RcBlock::new(callback));
    }
}
fn identity(session: &NSURLSession) -> String {
    session
        .configuration()
        .identifier()
        .map(|s| s.to_string())
        .unwrap_or_default()
}
fn session(id: &str) -> Retained<NSURLSession> {
    let mut state = state().lock().unwrap();
    if let Some(session) = state.sessions.get(id) {
        return session.clone();
    }
    let config = NSURLSessionConfiguration::backgroundSessionConfigurationWithIdentifier(
        &NSString::from_str(&format!("{PREFIX}{id}")),
    );
    config.setSharedContainerIdentifier(Some(&NSString::from_str(GROUP)));
    config.setDiscretionary(false);
    config.setSessionSendsLaunchEvents(true);
    let queue = NSOperationQueue::new();
    queue.setMaxConcurrentOperationCount(1);
    let delegate: Retained<UploadDelegate> = unsafe { msg_send![UploadDelegate::class(), new] };
    let result = unsafe {
        NSURLSession::sessionWithConfiguration_delegate_delegateQueue(
            &config,
            Some(ProtocolObject::from_ref(&*delegate)),
            Some(&queue),
        )
    };
    state.sessions.insert(id.into(), result.clone());
    result
}
fn next(id: &str) -> Result<()> {
    let auth = credentials(id, None)?.ok_or("请打开 ClipRoam 重新登录")?;
    let dir = directory(id)?;
    let request = super::dispatch(json!({"command":"next","directory":dir,"session":auth}))?;
    if request["complete"] == true {
        credentials(id, Some(None))?;
        notify();
        if let Some(session) = state().lock().unwrap().sessions.get(id) {
            session.finishTasksAndInvalidate();
        }
        return Ok(());
    }
    let url = NSURL::URLWithString(&NSString::from_str(
        request["url"].as_str().ok_or("上传地址缺失")?,
    ))
    .ok_or("上传地址无效")?;
    let request_obj = NSMutableURLRequest::requestWithURL(&url);
    request_obj.setHTTPMethod(&NSString::from_str(
        request["method"].as_str().unwrap_or("POST"),
    ));
    request_obj.setTimeoutInterval(120.0);
    request_obj.setValue_forHTTPHeaderField(
        Some(&NSString::from_str(&format!(
            "Bearer {}",
            auth["token"].as_str().unwrap_or("")
        ))),
        ns_string!("Authorization"),
    );
    request_obj.setValue_forHTTPHeaderField(
        Some(&NSString::from_str(
            request["contentType"]
                .as_str()
                .unwrap_or("application/json"),
        )),
        ns_string!("Content-Type"),
    );
    let file = NSURL::fileURLWithPath(&NSString::from_str(
        request["bodyPath"].as_str().ok_or("上传内容缺失")?,
    ));
    let task = session(id).uploadTaskWithRequest_fromFile(&request_obj, &file);
    task.setTaskDescription(Some(&NSString::from_str(id)));
    task.resume();
    Ok(())
}
fn fail(id: &str, error: &str) {
    if let Ok(dir) = directory(id) {
        let _ = super::dispatch(json!({"command":"fail","directory":dir,"error":error}));
        if !dir.join("upload.json").exists() {
            let _ = credentials(id, Some(None));
        }
    }
    notify();
}
fn finish_events(session: &NSURLSession) {
    let completion = state()
        .lock()
        .unwrap()
        .completions
        .remove(&identity(session));
    if let Some(completion) = completion {
        on_main(move || completion.call(()));
    }
}

define_class!(
    #[unsafe(super=NSObject)]
    #[name = "ClipRoamShareUploadDelegate"]
    struct UploadDelegate;
    unsafe impl NSObjectProtocol for UploadDelegate {}
    unsafe impl NSURLSessionDelegate for UploadDelegate {
        #[unsafe(method(URLSessionDidFinishEventsForBackgroundURLSession:))]
        fn finished(&self, session: &NSURLSession) {
            finish_events(session);
        }
        #[unsafe(method(URLSession:didBecomeInvalidWithError:))]
        fn invalid(&self, session: &NSURLSession, _error: Option<&NSError>) {
            finish_events(session);
            let id = identity(session).trim_start_matches(PREFIX).to_owned();
            state().lock().unwrap().sessions.remove(&id);
        }
    }
    unsafe impl NSURLSessionTaskDelegate for UploadDelegate {
        #[unsafe(method(URLSession:task:willPerformHTTPRedirection:newRequest:completionHandler:))]
        unsafe fn redirect(
            &self,
            _session: &NSURLSession,
            _task: &NSURLSessionTask,
            _response: &NSHTTPURLResponse,
            _request: &NSURLRequest,
            completion: &DynBlock<dyn Fn(*mut NSURLRequest)>,
        ) {
            completion.call((std::ptr::null_mut(),));
        }
        #[unsafe(method(URLSession:task:didCompleteWithError:))]
        fn complete(
            &self,
            session: &NSURLSession,
            task: &NSURLSessionTask,
            error: Option<&NSError>,
        ) {
            let key = format!("{}:{}", identity(session), task.taskIdentifier());
            let data = state()
                .lock()
                .unwrap()
                .responses
                .remove(&key)
                .unwrap_or_default();
            let Some(id) = task.taskDescription().map(|s| s.to_string()) else {
                return;
            };
            let result = (|| -> Result<()> {
                if let Some(error) = error {
                    return Err(error.localizedDescription().to_string());
                }
                let response = task
                    .response()
                    .and_then(|r| r.downcast::<NSHTTPURLResponse>().ok())
                    .ok_or("服务器响应缺失")?;
                super::dispatch(
                    json!({"command":"advance","directory":directory(&id)?,"status":response.statusCode(),"body":String::from_utf8_lossy(&data)}),
                )?;
                next(&id)
            })();
            if let Err(error) = result {
                fail(&id, &error);
            }
        }
    }
    unsafe impl NSURLSessionDataDelegate for UploadDelegate {
        #[unsafe(method(URLSession:dataTask:didReceiveData:))]
        fn data(&self, session: &NSURLSession, task: &NSURLSessionDataTask, data: &NSData) {
            let key = format!("{}:{}", identity(session), task.taskIdentifier());
            let mut state = state().lock().unwrap();
            let body = state.responses.entry(key).or_default();
            if body.len() + data.length() <= 1024 * 1024 {
                body.extend(unsafe { data.as_bytes_unchecked() });
            }
        }
    }
);
unsafe impl Send for UploadDelegate {}
unsafe impl Sync for UploadDelegate {}

pub fn configure(auth: Option<Value>) -> Result<()> {
    credentials("current", Some(auth.as_ref()))?;
    if let Some(auth) = auth {
        resume(&auth)?;
    }
    Ok(())
}
fn resume(auth: &Value) -> Result<()> {
    for entry in fs::read_dir(root()?).map_err(io)? {
        let dir = entry.map_err(io)?.path();
        let id = dir.file_name().unwrap().to_string_lossy().into_owned();
        let plan: Value = match fs::read(dir.join("upload.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
        {
            Some(p) => p,
            None => continue,
        };
        if plan["scope"] != auth["scope"] {
            continue;
        }
        if plan["status"] == "complete" {
            fs::remove_dir_all(&dir).map_err(io)?;
            credentials(&id, Some(None))?;
        } else if plan["status"] == "failed" {
            credentials(&id, Some(Some(auth)))?;
            if let Err(error) = next(&id) {
                fail(&id, &error);
            }
        }
    }
    Ok(())
}
pub fn pending(scope: &str) -> Result<Value> {
    let mut shares = Vec::new();
    for entry in fs::read_dir(root()?).map_err(io)? {
        let dir = entry.map_err(io)?.path();
        if dir.join("upload.json").exists() {
            continue;
        }
        let bytes = match fs::read(dir.join("share.json")) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let share: Value = serde_json::from_slice(&bytes).map_err(io)?;
        if !share["ownerScope"].is_null() && share["ownerScope"] != scope {
            continue;
        }
        let marker = dir.join("preparing");
        if marker.exists() {
            let age = marker
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if age < 300 {
                continue;
            }
            let _ = fs::remove_file(marker);
        }
        shares.push(share);
    }
    Ok(json!(shares))
}
pub fn acknowledge(id: &str) -> Result<()> {
    let dir = directory(id)?;
    credentials(id, Some(None))?;
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(io)?;
    }
    Ok(())
}

#[link(name = "objc")]
extern "C" {
    fn class_getInstanceMethod(class: *const AnyClass, selector: Sel) -> *mut c_void;
    fn method_getImplementation(method: *mut c_void) -> *const c_void;
    fn class_addMethod(
        class: *const AnyClass,
        selector: Sel,
        implementation: *const c_void,
        types: *const c_char,
    ) -> bool;
    fn method_setImplementation(
        method: *mut c_void,
        implementation: *const c_void,
    ) -> *const c_void;
}
static ORIGINAL: OnceLock<usize> = OnceLock::new();
unsafe extern "C" fn background(
    receiver: *mut AnyObject,
    selector: Sel,
    application: *mut AnyObject,
    identifier: *mut NSString,
    completion: *mut DynBlock<dyn Fn()>,
) {
    let name = (*identifier).to_string();
    if let Some(id) = name.strip_prefix(PREFIX) {
        if let Some(block) = RcBlock::copy(completion) {
            state()
                .lock()
                .unwrap()
                .completions
                .insert(name.clone(), block);
        }
        let _ = session(id);
    } else if let Some(original) = ORIGINAL.get().filter(|p| **p != 0) {
        let call: unsafe extern "C" fn(
            *mut AnyObject,
            Sel,
            *mut AnyObject,
            *mut NSString,
            *mut DynBlock<dyn Fn()>,
        ) = std::mem::transmute(*original);
        call(receiver, selector, application, identifier, completion)
    } else {
        (*completion).call(())
    }
}
pub unsafe fn attach() {
    static ATTACHED: OnceLock<()> = OnceLock::new();
    if ATTACHED.get().is_some() {
        return;
    }
    let app: Retained<AnyObject> = msg_send![class!(UIApplication), sharedApplication];
    let delegate: Option<Retained<AnyObject>> = msg_send![&*app, delegate];
    let Some(delegate) = delegate else { return };
    let class = delegate.class();
    let selector = sel!(application:handleEventsForBackgroundURLSession:completionHandler:);
    let method = class_getInstanceMethod(class, selector);
    let original = if method.is_null() {
        0
    } else {
        method_getImplementation(method) as usize
    };
    let _ = ORIGINAL.set(original);
    if !class_addMethod(
        class,
        selector,
        background as *const c_void,
        c"v@:@@@?".as_ptr(),
    ) {
        method_setImplementation(method, background as *const c_void);
    }
    let _: () = msg_send![&*app,setDelegate:&*delegate];
    let block = RcBlock::new(|_notification: *mut NSNotification| {
        if let Ok(Some(auth)) = credentials("current", None) {
            let _ = resume(&auth);
        }
        notify();
    });
    let center = NSNotificationCenter::defaultCenter();
    let observer: Retained<AnyObject> = msg_send![&*center,addObserverForName:ns_string!("UIApplicationDidBecomeActiveNotification"),object:std::ptr::null::<AnyObject>(),queue:&*NSOperationQueue::mainQueue(),usingBlock:&*block];
    state().lock().unwrap().observer = Some(observer);
    let _ = ATTACHED.set(());
}

struct Capture {
    controller: Retained<AnyObject>,
    providers: Vec<Retained<NSItemProvider>>,
    id: String,
    dir: PathBuf,
    content: Mutex<(Vec<String>, Vec<Value>)>,
}
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}
fn done(capture: Arc<Capture>, message: String) {
    on_main(move || unsafe {
        let alert: Retained<AnyObject> = msg_send![class!(UIAlertController),alertControllerWithTitle:ns_string!("分享到 ClipRoam"),message:&*NSString::from_str(&message),preferredStyle:1isize];
        let controller = capture.controller.clone();
        let action = RcBlock::new(move |_action: *mut AnyObject| {
            let context: Option<Retained<AnyObject>> = msg_send![&*controller, extensionContext];
            if let Some(context) = context {
                let _: () = msg_send![&*context,completeRequestReturningItems:std::ptr::null::<AnyObject>(),completionHandler:std::ptr::null::<c_void>()];
            }
        });
        let button: Retained<AnyObject> = msg_send![class!(UIAlertAction),actionWithTitle:ns_string!("完成"),style:0isize,handler:&*action];
        let _: () = msg_send![&*alert,addAction:&*button];
        let _: () = msg_send![&*capture.controller,presentViewController:&*alert,animated:true,completion:std::ptr::null::<c_void>()];
    });
}
fn commit(capture: Arc<Capture>) {
    std::thread::spawn(move || {
        let result = (|| -> Result<String> {
            let content = capture.content.lock().unwrap();
            if content.0.is_empty() && content.1.is_empty() {
                return Err("没有可接收的分享内容".into());
            }
            let auth = credentials("current", None).unwrap_or(None);
            let share = json!({"id":capture.id,"text":content.0.join("\n"),"html":null,"items":content.1,"ownerScope":auth.as_ref().map(|a|&a["scope"])});
            if let Some(auth) = &auth {
                credentials(&capture.id, Some(Some(auth)))?;
                fs::write(capture.dir.join("preparing"), []).map_err(io)?;
            }
            super::atomic(
                &capture.dir.join("share.json"),
                &serde_json::to_vec(&share).map_err(io)?,
            )?;
            let Some(auth) = auth else {
                return Ok("内容已保存，请打开 ClipRoam 登录后上传。".into());
            };
            super::dispatch(
                json!({"command":"prepare","directory":capture.dir,"session":auth,"share":share}),
            )?;
            next(&capture.id)?;
            Ok("已开始后台上传，无需打开 ClipRoam。".into())
        })();
        let _ = fs::remove_file(capture.dir.join("preparing"));
        let message = match result {
            Ok(m) => m,
            Err(e) => {
                fail(&capture.id, &e);
                format!("内容已保留。{e}")
            }
        };
        notify();
        done(capture, message);
    });
}
#[link(name = "UniformTypeIdentifiers", kind = "framework")]
extern "C" {
    static UTTypeData: *const AnyObject;
    static UTTypeText: *const AnyObject;
    static UTTypeURL: *const AnyObject;
}
fn receive(capture: Arc<Capture>, index: usize) {
    unsafe {
        let Some(provider) = capture.providers.get(index) else {
            commit(capture);
            return;
        };
        let types = provider.registeredTypeIdentifiers();
        let mut selected = None;
        for identifier in types.iter() {
            let uti: Option<Retained<AnyObject>> =
                msg_send![class!(UTType),typeWithIdentifier:&*identifier];
            if let Some(uti) = uti {
                let data: bool = msg_send![&*uti,conformsToType:&*UTTypeData];
                let text: bool = msg_send![&*uti,conformsToType:&*UTTypeText];
                let url: bool = msg_send![&*uti,conformsToType:&*UTTypeURL];
                if data && !text && !url {
                    selected = Some(identifier);
                    break;
                }
            }
        }
        let callback_capture = capture.clone();
        if let Some(identifier) = selected {
            let block = RcBlock::new(move |url: *mut NSURL, error: *mut NSError| {
                if !error.is_null() || url.is_null() {
                    done(
                        callback_capture.clone(),
                        "无法读取分享文件，请重新分享。".into(),
                    );
                    return;
                }
                let result = (|| -> Result<()> {
                    let source = (*url).path().ok_or("分享文件地址无效")?.to_string();
                    let name = Path::new(&source)
                        .file_name()
                        .ok_or("分享文件名无效")?
                        .to_string_lossy()
                        .into_owned();
                    let dir = callback_capture.dir.join(index.to_string());
                    fs::create_dir_all(&dir).map_err(io)?;
                    let target = dir.join(&name);
                    fs::copy(source, &target).map_err(io)?;
                    callback_capture.content.lock().unwrap().1.push(
                        json!({"path":target,"name":name,"mimeType":"application/octet-stream"}),
                    );
                    Ok(())
                })();
                match result {
                    Ok(()) => receive(callback_capture.clone(), index + 1),
                    Err(e) => done(callback_capture.clone(), e),
                }
            });
            let _ = provider
                .loadFileRepresentationForTypeIdentifier_completionHandler(&identifier, &block);
        } else {
            let url = provider.hasItemConformingToTypeIdentifier(ns_string!("public.url"));
            let identifier = if url {
                ns_string!("public.url")
            } else {
                ns_string!("public.plain-text")
            };
            let block = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
                if !error.is_null() || value.is_null() {
                    done(
                        callback_capture.clone(),
                        "无法读取分享文字或链接，请重新分享。".into(),
                    );
                    return;
                }
                let value = &*value;
                if let Some(url) = value.downcast_ref::<NSURL>() {
                    if url.isFileURL() {
                        let access = url.startAccessingSecurityScopedResource();
                        let result = (|| -> Result<()> {
                            let source = url.path().ok_or("文件地址无效")?.to_string();
                            let name = Path::new(&source)
                                .file_name()
                                .ok_or("文件名无效")?
                                .to_string_lossy()
                                .into_owned();
                            let target = callback_capture.dir.join(index.to_string()).join(&name);
                            fs::create_dir_all(target.parent().unwrap()).map_err(io)?;
                            fs::copy(source, &target).map_err(io)?;
                            callback_capture.content.lock().unwrap().1.push(json!({"path":target,"name":name,"mimeType":"application/octet-stream"}));
                            Ok(())
                        })();
                        if access {
                            url.stopAccessingSecurityScopedResource()
                        }
                        match result {
                            Ok(()) => receive(callback_capture.clone(), index + 1),
                            Err(e) => done(callback_capture.clone(), e),
                        };
                        return;
                    }
                    if let Some(text) = url.absoluteString() {
                        callback_capture
                            .content
                            .lock()
                            .unwrap()
                            .0
                            .push(text.to_string());
                    }
                } else if let Some(text) = value.downcast_ref::<NSString>() {
                    callback_capture
                        .content
                        .lock()
                        .unwrap()
                        .0
                        .push(text.to_string());
                } else if let Some(data) = value.downcast_ref::<NSData>() {
                    callback_capture
                        .content
                        .lock()
                        .unwrap()
                        .0
                        .push(String::from_utf8_lossy(data.as_bytes_unchecked()).into_owned());
                }
                receive(callback_capture.clone(), index + 1);
            });
            let _: () = msg_send![&**provider,loadItemForTypeIdentifier:identifier,options:std::ptr::null::<AnyObject>(),completionHandler:&*block];
        }
    }
}
pub fn dispatch(input: Value) -> Result<Value> {
    if input["command"] != "ios.start" {
        return Err("未知 iOS 分享操作".into());
    }
    let pointer = input["controller"].as_u64().ok_or("缺少分享控制器")? as *mut AnyObject;
    let controller = unsafe { Retained::retain(pointer) }.ok_or("分享控制器无效")?;
    let providers = unsafe {
        let context: Retained<AnyObject> = msg_send![&*controller, extensionContext];
        let items: Retained<NSArray<NSExtensionItem>> = msg_send![&*context, inputItems];
        items
            .iter()
            .flat_map(|item| {
                item.attachments()
                    .map(|a| a.iter().collect::<Vec<_>>())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
    };
    let id = NSUUID::UUID().UUIDString().to_string().to_lowercase();
    let dir = directory(&id)?;
    fs::create_dir_all(&dir).map_err(io)?;
    receive(
        Arc::new(Capture {
            controller,
            providers,
            id,
            dir,
            content: Mutex::new((Vec::new(), Vec::new())),
        }),
        0,
    );
    Ok(json!({}))
}
