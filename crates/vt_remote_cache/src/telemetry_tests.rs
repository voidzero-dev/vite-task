//! The public client's network trace context must identify its exported spans.

use std::{
    net::TcpListener,
    sync::{Arc, mpsc},
    time::Duration,
};

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig as _};
use opentelemetry_sdk::{
    Resource,
    trace::{BatchConfigBuilder, BatchSpanProcessor, SdkTracerProvider},
};

use crate::{Client, Error, auth::Anonymous, test_server::serve_once};

#[tokio::test]
async fn exports_cache_requests_with_their_propagated_identity_and_no_secrets() {
    let cache = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = vt_str::format!(
        "http://account:password@{}/namespace-secret?token=cache-secret",
        cache.local_addr().unwrap()
    );
    let cache_server = std::thread::spawn(move || {
        [
            ("HTTP/1.1 404 Not Found", b"".as_slice()),
            ("HTTP/1.1 200 OK", b"".as_slice()),
            ("HTTP/1.1 200 OK", b"blob-secret".as_slice()),
            ("HTTP/1.1 200 OK", b"blob-secret".as_slice()),
            ("HTTP/1.1 503 Service Unavailable", b"response-secret".as_slice()),
        ]
        .into_iter()
        .map(|(status, body)| serve_once(&cache, status, body))
        .collect::<Vec<_>>()
    });
    let collector = TcpListener::bind("127.0.0.1:0").unwrap();
    let collector_url = vt_str::format!("http://{}/v1/traces", collector.local_addr().unwrap());
    let (export_tx, export_rx) = mpsc::channel();
    std::thread::spawn(move || {
        export_tx.send(serve_once(&collector, "HTTP/1.1 200 OK", b"{}")).unwrap();
    });
    let provider =
        tokio::task::spawn_blocking(move || collector_provider(&collector_url)).await.unwrap();
    let client = Client::new(&endpoint, Arc::new(Anonymous))
        .unwrap()
        .with_tracer(provider.tracer("cache-client-test"));

    assert!(client.fetch(b"key-secret", b"fallback-secret").await.unwrap().is_none());
    client.store(b"key-secret", b"fallback-secret", b"body-secret", None).await.unwrap();
    let mut download = client.download("blob-secret").await.unwrap();
    assert_eq!(download.chunk().await.unwrap().unwrap(), b"blob-secret".as_slice());
    assert!(download.chunk().await.unwrap().is_none());
    // Abandoning the stream also ends its span, rather than losing the edge.
    drop(client.download("blob-secret").await.unwrap());
    assert!(matches!(client.fetch(b"key-secret", b"").await, Err(Error::Status(..))));

    tokio::task::spawn_blocking(move || provider.shutdown()).await.unwrap().unwrap();
    let requests = cache_server.join().unwrap();
    let export =
        export_rx.recv_timeout(Duration::from_secs(5)).expect("exporter flushed its spans");
    let body = export.windows(4).position(|window| window == b"\r\n\r\n").unwrap() + 4;
    let body = &export[body..];
    let raw = std::str::from_utf8(body).unwrap();
    for secret in [
        "account",
        "password",
        "namespace-secret",
        "cache-secret",
        "key-secret",
        "fallback-secret",
        "body-secret",
        "blob-secret",
        "response-secret",
    ] {
        assert!(!raw.contains(secret), "export contained {secret}");
    }
    let export: serde_json::Value = serde_json::from_slice(body).unwrap();
    let resource = &export["resourceSpans"][0];
    assert_eq!(
        resource["resource"]["attributes"][0],
        serde_json::json!({"key": "service.name", "value": {"stringValue": "cache-client-test"}})
    );
    let spans = resource["scopeSpans"][0]["spans"].as_array().unwrap();
    assert_eq!(spans.len(), 5);
    for (request, (name, status, failed)) in requests.iter().zip([
        ("POST /fetch", 404, false),
        ("POST /store", 200, false),
        ("GET /blob", 200, false),
        ("GET /blob", 200, false),
        ("POST /fetch", 503, true),
    ]) {
        let header_end = request.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
        let request = std::str::from_utf8(&request[..header_end]).unwrap();
        let context = request
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("traceparent").then_some(value.trim())
            })
            .unwrap();
        let context = context.split('-').collect::<Vec<_>>();
        assert_eq!(context[0], "00");
        assert_eq!(context[3], "01");
        let span = spans.iter().find(|span| span["spanId"] == context[2]).unwrap();
        assert_eq!(span["traceId"], context[1]);
        assert_eq!(span["name"], name);
        assert_eq!(span["kind"], 3);
        let attributes = span["attributes"].as_array().unwrap();
        let status_attribute = attributes
            .iter()
            .find(|attribute| attribute["key"] == "http.response.status_code")
            .unwrap();
        assert_eq!(status_attribute["value"]["intValue"], status.to_string());
        assert_eq!(span["status"]["code"].as_i64().unwrap_or(0) == 2, failed);
        assert_ne!(span["endTimeUnixNano"], "0");
    }
}

fn collector_provider(endpoint: &str) -> SdkTracerProvider {
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(endpoint.to_owned())
        .with_protocol(Protocol::HttpJson)
        .build()
        .unwrap();
    let processor = BatchSpanProcessor::builder(exporter)
        .with_batch_config(
            BatchConfigBuilder::default().with_scheduled_delay(Duration::from_secs(60)).build(),
        )
        .build();
    SdkTracerProvider::builder()
        .with_span_processor(processor)
        .with_resource(Resource::builder_empty().with_service_name("cache-client-test").build())
        .build()
}
