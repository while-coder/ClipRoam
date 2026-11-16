//! Apple uses the system Bonjour daemon, so browsing the declared service does
//! not require iOS's restricted raw-multicast entitlement.
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[path = "bonjour.rs"]
mod implementation;
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(super) use implementation::{start, Discovery};

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
mod implementation {
    use super::super::*;
    #[cfg(target_os = "android")]
    use super::android_multicast;
    use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
    pub(crate) struct Discovery(ServiceDaemon);
    impl Discovery {
        pub(crate) fn shutdown(self) {
            let _ = self.0.shutdown();
            #[cfg(target_os = "android")]
            android_multicast(false, None);
        }
    }
    pub(crate) fn start(service: &Arc<Service>) -> Result<Discovery, String> {
        let daemon = ServiceDaemon::new().map_err(err)?;
        let result = (|| {
            let device = &service.device;
            let info = ServiceInfo::new(
                SERVICE,
                &device.id,
                &format!("{}.local.", device.id),
                "",
                service.port,
                [
                    ("name", device.name.as_str()),
                    ("id", device.id.as_str()),
                    ("version", "1"),
                ]
                .as_slice(),
            )
            .map_err(err)?
            .enable_addr_auto();
            daemon.register(info).map_err(err)?;
            daemon.browse(SERVICE).map_err(err)
        })();
        let events = match result {
            Ok(events) => events,
            Err(e) => {
                let _ = daemon.shutdown();
                return Err(e);
            }
        };
        let weak = Arc::downgrade(service);
        let cancel = service.cancel.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    event = events.recv_async() => {
                        let Some(service) = weak.upgrade() else { break; };
                        match event {
                            Ok(ServiceEvent::ServiceResolved(info)) => {
                                if info.get_property_val_str("id") == Some(service.device.id.as_str()) { continue; }
                                let name = info.get_property_val_str("name").unwrap_or("ClipRoam").chars().take(40).collect::<String>();
                                let peers = info.get_addresses().iter().filter_map(|ip| match ip { IpAddr::V4(ip) if !ip.is_loopback() => Some(Peer { address: format!("{ip}:{}", info.get_port()), name: name.clone() }), _ => None }).collect();
                                let mut inner = service.inner.lock().unwrap();
                                if inner.peers.len() < 128 || inner.peers.contains_key(info.get_fullname()) { inner.peers.insert(info.get_fullname().into(), peers); }
                            }
                            Ok(ServiceEvent::ServiceRemoved(_, fullname)) => { service.inner.lock().unwrap().peers.remove(&fullname); }
                            Err(e) => { *service.discovery_error.lock().unwrap() = Some(format!("自动发现失败，可手动连接：{e}")); break; }
                            _ => ()
                        }
                    }
                }
            }
        });
        #[cfg(target_os = "android")]
        android_multicast(true, Some(Arc::downgrade(service)));
        Ok(Discovery(daemon))
    }
}
#[cfg(not(any(target_os = "macos", target_os = "ios")))]
pub(super) use implementation::{start, Discovery};

#[cfg(target_os = "android")]
fn android_multicast(enabled: bool, service: Option<std::sync::Weak<super::Service>>) {
    tauri::wry::prelude::dispatch(move |env, activity, _| {
        let result = tauri::wry::prelude::find_class(
            env,
            activity,
            "com/whilecode/cliproam/LanDiscovery".into(),
        )
        .and_then(|class| {
            env.call_static_method(
                class,
                "setEnabled",
                "(Landroid/content/Context;Z)V",
                &[
                    jni::objects::JValue::Object(activity),
                    jni::objects::JValue::Bool(enabled.into()),
                ],
            )
            .map(|_| ())
        });
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
        if let Err(e) = result {
            log::warn!("[lan] multicast lock failed: {e}");
            if let Some(service) = service.and_then(|s| s.upgrade()) {
                *service.discovery_error.lock().unwrap() =
                    Some("无法启用 Wi-Fi 自动发现，可手动输入 IP 连接".into());
            }
        }
    });
}
