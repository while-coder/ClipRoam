use super::super::{err, Peer, Service};
use std::{
    ffi::{c_char, c_void, CStr, CString},
    ptr,
    sync::{mpsc, Arc, Weak},
    time::{Duration, Instant},
};

type DnsRef = *mut c_void;
type RegisterCallback = unsafe extern "C" fn(
    DnsRef,
    u32,
    i32,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);
type BrowseCallback = unsafe extern "C" fn(
    DnsRef,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    *const c_char,
    *mut c_void,
);
type ResolveCallback = unsafe extern "C" fn(
    DnsRef,
    u32,
    u32,
    i32,
    *const c_char,
    *const c_char,
    u16,
    u16,
    *const u8,
    *mut c_void,
);
type AddressCallback = unsafe extern "C" fn(
    DnsRef,
    u32,
    u32,
    i32,
    *const c_char,
    *const libc::sockaddr,
    u32,
    *mut c_void,
);
// DNS-SD is re-exported by libSystem on Apple platforms (modern SDKs do not
// provide a standalone libdns_sd link stub).
#[link(name = "System")]
extern "C" {
    fn DNSServiceRegister(
        reference: *mut DnsRef,
        flags: u32,
        interface: u32,
        name: *const c_char,
        kind: *const c_char,
        domain: *const c_char,
        host: *const c_char,
        port: u16,
        txt_len: u16,
        txt: *const c_void,
        callback: RegisterCallback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceBrowse(
        reference: *mut DnsRef,
        flags: u32,
        interface: u32,
        kind: *const c_char,
        domain: *const c_char,
        callback: BrowseCallback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceResolve(
        reference: *mut DnsRef,
        flags: u32,
        interface: u32,
        name: *const c_char,
        kind: *const c_char,
        domain: *const c_char,
        callback: ResolveCallback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceGetAddrInfo(
        reference: *mut DnsRef,
        flags: u32,
        interface: u32,
        protocol: u32,
        hostname: *const c_char,
        callback: AddressCallback,
        context: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(reference: DnsRef) -> i32;
    fn DNSServiceProcessResult(reference: DnsRef) -> i32;
    fn DNSServiceRefDeallocate(reference: DnsRef);
}
// All DNS refs and callback contexts stay on one dedicated thread. Contexts
// outlive their refs, and no DNSService call races with deallocation.
struct Operation {
    raw: DnsRef,
    context: Box<Context>,
    expires: Option<Instant>,
}
impl Drop for Operation {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                DNSServiceRefDeallocate(self.raw);
            }
        }
    }
}
struct Context {
    sender: mpsc::Sender<Event>,
    key: String,
    label: String,
    port: u16,
}
enum Event {
    Found {
        name: String,
        kind: String,
        domain: String,
        interface: u32,
        key: String,
    },
    Removed(String),
    Resolved {
        key: String,
        host: String,
        port: u16,
        interface: u32,
        label: String,
    },
    Address {
        key: String,
        address: String,
        label: String,
        add: bool,
    },
    Error(i32),
}
unsafe fn string(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
unsafe extern "C" fn registered(
    _: DnsRef,
    _: u32,
    error: i32,
    _: *const c_char,
    _: *const c_char,
    _: *const c_char,
    context: *mut c_void,
) {
    if error != 0 {
        let _ = (*(context as *mut Context))
            .sender
            .send(Event::Error(error));
    }
}
unsafe extern "C" fn browsed(
    _: DnsRef,
    flags: u32,
    interface: u32,
    error: i32,
    name: *const c_char,
    kind: *const c_char,
    domain: *const c_char,
    context: *mut c_void,
) {
    let c = &*(context as *mut Context);
    if error != 0 {
        let _ = c.sender.send(Event::Error(error));
        return;
    }
    let name = string(name);
    let key = format!("{name}:{interface}");
    let event = if flags & 2 != 0 {
        Event::Found {
            name,
            kind: string(kind),
            domain: string(domain),
            interface,
            key,
        }
    } else {
        Event::Removed(key)
    };
    let _ = c.sender.send(event);
}
unsafe extern "C" fn resolved(
    _: DnsRef,
    _: u32,
    interface: u32,
    error: i32,
    _: *const c_char,
    host: *const c_char,
    port: u16,
    txt_len: u16,
    txt: *const u8,
    context: *mut c_void,
) {
    let c = &*(context as *mut Context);
    if error != 0 {
        let _ = c.sender.send(Event::Error(error));
        return;
    }
    let mut label = c.label.clone();
    if !txt.is_null() {
        let mut bytes = std::slice::from_raw_parts(txt, txt_len as usize);
        while let Some((&len, rest)) = bytes.split_first() {
            if rest.len() < len as usize {
                break;
            }
            let (item, remaining) = rest.split_at(len as usize);
            bytes = remaining;
            if let Some(name) = item.strip_prefix(b"name=") {
                label = String::from_utf8_lossy(name).chars().take(40).collect();
            }
        }
    }
    let _ = c.sender.send(Event::Resolved {
        key: c.key.clone(),
        host: string(host),
        port: u16::from_be(port),
        interface,
        label,
    });
}
unsafe extern "C" fn addressed(
    _: DnsRef,
    flags: u32,
    _: u32,
    error: i32,
    _: *const c_char,
    address: *const libc::sockaddr,
    _: u32,
    context: *mut c_void,
) {
    let c = &*(context as *mut Context);
    if error != 0 {
        let _ = c.sender.send(Event::Error(error));
        return;
    }
    if address.is_null() || (*address).sa_family as i32 != libc::AF_INET {
        return;
    }
    let ip = std::net::Ipv4Addr::from(
        (*(address as *const libc::sockaddr_in))
            .sin_addr
            .s_addr
            .to_ne_bytes(),
    );
    if ip.is_loopback() || ip.is_unspecified() {
        return;
    }
    let _ = c.sender.send(Event::Address {
        key: c.key.clone(),
        address: format!("{ip}:{}", c.port),
        label: c.label.clone(),
        add: flags & 2 != 0,
    });
}
fn operation(sender: &mpsc::Sender<Event>, key: String, label: String, port: u16) -> Operation {
    Operation {
        raw: ptr::null_mut(),
        context: Box::new(Context {
            sender: sender.clone(),
            key,
            label,
            port,
        }),
        expires: None,
    }
}
fn check(error: i32) -> Result<(), String> {
    if error == 0 {
        Ok(())
    } else {
        Err(format!("Bonjour 错误 {error}，请检查本地网络权限"))
    }
}
fn context(op: &mut Operation) -> *mut c_void {
    (&mut *op.context) as *mut Context as *mut c_void
}

pub(crate) struct Discovery;
impl Discovery {
    pub(crate) fn shutdown(self) {}
}
pub(crate) fn start(service: &Arc<Service>) -> Result<Discovery, String> {
    let weak = Arc::downgrade(service);
    let cancel = service.cancel.clone();
    let id = service.device.id.clone();
    let label = service.device.name.clone();
    let port = service.port;
    std::thread::Builder::new()
        .name("cliproam-bonjour".into())
        .spawn(move || {
            if let Err(e) = run(&weak, &cancel, &id, &label, port) {
                if let Some(service) = weak.upgrade() {
                    *service.discovery_error.lock().unwrap() =
                        Some(format!("自动发现不可用，可手动连接：{e}"));
                }
            }
        })
        .map_err(err)?;
    Ok(Discovery)
}
fn run(
    weak: &Weak<Service>,
    cancel: &tokio_util::sync::CancellationToken,
    id: &str,
    label: &str,
    port: u16,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let kind = CString::new("_cliproam._tcp").unwrap();
    let domain = CString::new("local.").unwrap();
    let instance = CString::new(id).map_err(err)?;
    let mut txt = Vec::new();
    for item in [
        format!("name={label}"),
        format!("id={id}"),
        "version=1".into(),
    ] {
        txt.push(item.len() as u8);
        txt.extend_from_slice(item.as_bytes());
    }
    let mut register = operation(&tx, String::new(), label.into(), port);
    let ctx = context(&mut register);
    check(unsafe {
        DNSServiceRegister(
            &mut register.raw,
            0,
            0,
            instance.as_ptr(),
            kind.as_ptr(),
            domain.as_ptr(),
            ptr::null(),
            port.to_be(),
            txt.len() as u16,
            txt.as_ptr().cast(),
            registered,
            ctx,
        )
    })?;
    let mut browse = operation(&tx, String::new(), String::new(), 0);
    let ctx = context(&mut browse);
    check(unsafe {
        DNSServiceBrowse(
            &mut browse.raw,
            0,
            0,
            kind.as_ptr(),
            domain.as_ptr(),
            browsed,
            ctx,
        )
    })?;
    let mut ops = vec![register, browse];
    while !cancel.is_cancelled() {
        let mut fds: Vec<_> = ops
            .iter()
            .map(|op| libc::pollfd {
                fd: unsafe { DNSServiceRefSockFD(op.raw) },
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        let count = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 200) };
        if count < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err(e));
        }
        for (op, fd) in ops.iter().zip(&fds) {
            if fd.revents & libc::POLLIN != 0 {
                check(unsafe { DNSServiceProcessResult(op.raw) })?;
            } else if fd.revents != 0 {
                return Err("Bonjour 连接中断".into());
            }
        }
        while let Ok(event) = rx.try_recv() {
            match event {
                Event::Found {
                    name,
                    kind,
                    domain,
                    interface,
                    key,
                } => {
                    if name == id || ops.len() >= 130 || ops.iter().any(|op| op.context.key == key)
                    {
                        continue;
                    }
                    let mut op = operation(&tx, key, name.clone(), 0);
                    op.expires = Some(Instant::now() + Duration::from_secs(15));
                    let (name, kind, domain) = (
                        CString::new(name).map_err(err)?,
                        CString::new(kind).map_err(err)?,
                        CString::new(domain).map_err(err)?,
                    );
                    let ctx = context(&mut op);
                    if unsafe {
                        DNSServiceResolve(
                            &mut op.raw,
                            0,
                            interface,
                            name.as_ptr(),
                            kind.as_ptr(),
                            domain.as_ptr(),
                            resolved,
                            ctx,
                        )
                    } == 0
                    {
                        ops.push(op);
                    }
                }
                Event::Removed(key) => {
                    ops.retain(|op| op.context.key != key);
                    if let Some(service) = weak.upgrade() {
                        service.inner.lock().unwrap().peers.remove(&key);
                    }
                }
                Event::Resolved {
                    key,
                    host,
                    port,
                    interface,
                    label,
                } => {
                    ops.retain(|op| op.context.key != key);
                    let mut op = operation(&tx, key, label, port);
                    let host = CString::new(host).map_err(err)?;
                    let ctx = context(&mut op);
                    if unsafe {
                        DNSServiceGetAddrInfo(
                            &mut op.raw,
                            0,
                            interface,
                            1,
                            host.as_ptr(),
                            addressed,
                            ctx,
                        )
                    } == 0
                    {
                        ops.push(op);
                    }
                }
                Event::Address {
                    key,
                    address,
                    label,
                    add,
                } => {
                    if let Some(service) = weak.upgrade() {
                        let mut inner = service.inner.lock().unwrap();
                        let peers = inner.peers.entry(key).or_default();
                        peers.retain(|p| p.address != address);
                        if add && peers.len() < 16 {
                            peers.push(Peer {
                                address,
                                name: label,
                            });
                        }
                    }
                }
                Event::Error(error) => {
                    if let Some(service) = weak.upgrade() {
                        *service.discovery_error.lock().unwrap() = Some(format!(
                            "Bonjour 错误 {error}，请检查本地网络权限；可手动连接"
                        ));
                    }
                }
            }
        }
        ops.retain(|op| op.expires.is_none_or(|deadline| deadline > Instant::now()));
    }
    Ok(())
}
