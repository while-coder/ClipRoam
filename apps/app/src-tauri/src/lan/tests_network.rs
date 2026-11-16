use super::*;

struct Fixture {
    service: Arc<Service>,
    address: SocketAddr,
    certificate: Vec<u8>,
}
impl Fixture {
    async fn new() -> Self {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let root = std::env::temp_dir().join(format!("cliproam-lan-test-{}", Uuid::new_v4()));
        tokio::fs::create_dir_all(root.join("inbox")).await.unwrap();
        let cert = rcgen::generate_simple_self_signed(vec!["cliproam.local".into()]).unwrap();
        let certificate = cert.cert.der().to_vec();
        let tls = axum_server::tls_rustls::RustlsConfig::from_der(
            vec![certificate.clone()],
            cert.key_pair.serialize_der(),
        )
        .await
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let service = Arc::new(Service {
            device: Device {
                id: Uuid::new_v4().to_string(),
                name: "测试设备".into(),
                protocol: 1,
                code: code(&certificate),
            },
            port: address.port(),
            root,
            inner: Mutex::new(Inner::default()),
            cancel: CancellationToken::new(),
            server: axum_server::Handle::new(),
            discovery: Mutex::new(None),
            discovery_error: Mutex::new(None),
        });
        let app = router(&service);
        let handle = service.server.clone();
        tokio::spawn(async move {
            axum_server::from_tcp_rustls(listener, tls)
                .handle(handle)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });
        Self {
            service,
            address,
            certificate,
        }
    }
    async fn decide(&self, accept: bool) -> String {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                {
                    let mut inner = self.service.inner.lock().unwrap();
                    if let Some((id, pending)) =
                        inner.pending.iter_mut().find(|(_, p)| p.decision.is_some())
                    {
                        pending.decision.take().unwrap().send(accept).unwrap();
                        return id.clone();
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("incoming offer timed out")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.cancel.cancel();
        self.service.server.shutdown();
        let _ = std::fs::remove_dir_all(&self.service.root);
    }
}

#[test]
fn https_text_and_large_file_round_trip_with_consent() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let receiver = Fixture::new().await;
        let sender = Fixture::new().await;
        let connected = connect(&sender.service, receiver.address.to_string())
            .await
            .unwrap();
        assert_eq!(connected.device.code, receiver.service.device.code);
        let service = sender.service.clone();
        let id = connected.connection_id.clone();
        let task = tokio::spawn(async move {
            send(&service, id, Some("你好，局域网！".into()), None).await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            receiver.service.inner.lock().unwrap().records.is_empty(),
            "No file is created before consent"
        );
        let receipt_id = receiver.decide(true).await;
        task.await.unwrap().unwrap();
        let receipt_path = receiver
            .service
            .root
            .join("inbox")
            .join(&receipt_id)
            .join("receipt.json");
        let record: Record =
            serde_json::from_slice(&tokio::fs::read(receipt_path).await.unwrap()).unwrap();
        assert_eq!(record.text.as_deref(), Some("你好，局域网！"));
        assert_eq!(
            tokio::fs::read(record.path.unwrap()).await.unwrap(),
            "你好，局域网！".as_bytes()
        );
        // Larger than Axum's default body limit and our metadata limit. Use an
        // internal metadata filename to ensure payloads cannot overwrite receipts.
        let source = sender.service.root.join("receipt.json");
        let content = (0..3 * 1024 * 1024)
            .map(|i| (i % 251) as u8)
            .collect::<Vec<_>>();
        tokio::fs::write(&source, &content).await.unwrap();
        let service = sender.service.clone();
        let connection = connected.connection_id;
        let task = tokio::spawn(async move {
            send(
                &service,
                connection,
                None,
                Some(source.to_string_lossy().into_owned()),
            )
            .await
        });
        let id = receiver.decide(true).await;
        task.await.unwrap().unwrap();
        assert_eq!(
            tokio::fs::read(
                receiver
                    .service
                    .root
                    .join("inbox")
                    .join(id)
                    .join("files/receipt.json")
            )
            .await
            .unwrap(),
            content
        );
    });
}

#[test]
fn rejection_and_expired_permits_never_write_files() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let receiver = Fixture::new().await;
        let sender = Fixture::new().await;
        let connected = connect(&sender.service, receiver.address.to_string())
            .await
            .unwrap();
        let service = sender.service.clone();
        let connection_id = connected.connection_id.clone();
        let task = tokio::spawn(async move {
            send(&service, connection_id, Some("reject me".into()), None).await
        });
        let id = receiver.decide(false).await;
        assert!(task.await.unwrap().is_err());
        assert!(!receiver.service.root.join("inbox").join(id).exists());
        let connection = sender
            .service
            .inner
            .lock()
            .unwrap()
            .connections
            .get(&connected.connection_id)
            .unwrap()
            .clone();
        let client = connection.client;
        let response = client
            .put(format!("{}/v1/upload/{}", connection.base, Uuid::new_v4()))
            .body("bad")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let id = Uuid::new_v4().to_string();
        receiver.service.inner.lock().unwrap().pending.insert(
            id.clone(),
            Pending {
                offer: Offer {
                    sender: "sender".into(),
                    kind: "file".into(),
                    name: "safe.txt".into(),
                    size: 3,
                },
                ip: "127.0.0.1".parse().unwrap(),
                expires: Instant::now() - Duration::from_secs(1),
                decision: None,
                token: Some("expired".into()),
            },
        );
        let response = client
            .put(format!("{}/v1/upload/{id}", connection.base))
            .bearer_auth("expired")
            .body("bad")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(!receiver.service.root.join("inbox").join(id).exists());
    });
}

#[test]
fn truncated_upload_is_cleaned_and_permit_is_single_use() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let receiver = Fixture::new().await;
        let sender = Fixture::new().await;
        let connected = connect(&sender.service, receiver.address.to_string())
            .await
            .unwrap();
        let connection = sender
            .service
            .inner
            .lock()
            .unwrap()
            .connections
            .get(&connected.connection_id)
            .unwrap()
            .clone();
        let offer = Offer {
            sender: "sender".into(),
            kind: "file".into(),
            name: "safe.txt".into(),
            size: 10,
        };
        let cloned = connection.clone();
        let task = tokio::spawn(async move {
            small_json::<Permit>(
                cloned
                    .client
                    .post(format!("{}/v1/offers", cloned.base))
                    .json(&offer)
                    .send()
                    .await
                    .unwrap(),
            )
            .await
            .unwrap()
        });
        let id = receiver.decide(true).await;
        let permit = task.await.unwrap();
        let response = connection
            .client
            .put(format!("{}/v1/upload/{id}", connection.base))
            .bearer_auth(&permit.token)
            .body("short")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            !receiver.service.root.join("inbox").join(&id).exists(),
            "partial directory must be removed"
        );
        let response = connection
            .client
            .put(format!("{}/v1/upload/{id}", connection.base))
            .bearer_auth(permit.token)
            .body("0123456789")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    });
}

#[test]
fn pinned_client_rejects_another_devices_certificate() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let receiver = Fixture::new().await;
        let other = Fixture::new().await;
        let client = client_builder()
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_der(&receiver.certificate).unwrap())
            .resolve("cliproam.local", other.address)
            .build()
            .unwrap();
        assert!(client
            .get(format!(
                "https://cliproam.local:{}/v1/info",
                other.address.port()
            ))
            .send()
            .await
            .is_err());
    });
}

#[test]
fn permits_require_the_approved_ip_and_token() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let receiver = Fixture::new().await;
        let sender = Fixture::new().await;
        let connected = connect(&sender.service, receiver.address.to_string())
            .await
            .unwrap();
        let connection = sender
            .service
            .inner
            .lock()
            .unwrap()
            .connections
            .get(&connected.connection_id)
            .unwrap()
            .clone();
        let id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string();
        receiver.service.inner.lock().unwrap().pending.insert(
            id.clone(),
            Pending {
                offer: Offer {
                    sender: "sender".into(),
                    kind: "file".into(),
                    name: "safe.txt".into(),
                    size: 3,
                },
                ip: "192.168.1.123".parse().unwrap(),
                expires: Instant::now() + Duration::from_secs(60),
                decision: None,
                token: Some(token.clone()),
            },
        );
        let url = format!("{}/v1/upload/{id}", connection.base);
        let response = connection
            .client
            .put(&url)
            .bearer_auth(&token)
            .body("yes")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        receiver
            .service
            .inner
            .lock()
            .unwrap()
            .pending
            .get_mut(&id)
            .unwrap()
            .ip = "127.0.0.1".parse().unwrap();
        let response = connection
            .client
            .put(&url)
            .bearer_auth("wrong-token")
            .body("yes")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(receiver.service.inner.lock().unwrap().records.is_empty());
        let response = connection
            .client
            .put(&url)
            .bearer_auth(&token)
            .body("yes")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            tokio::fs::read(
                receiver
                    .service
                    .root
                    .join("inbox")
                    .join(id)
                    .join("files/safe.txt")
            )
            .await
            .unwrap(),
            b"yes"
        );
    });
}
