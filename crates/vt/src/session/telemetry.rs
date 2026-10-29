//! A session owns its exporter, and drains it before returning an exit status.

use std::env;

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig as _};
use opentelemetry_sdk::{
    Resource,
    resource::{EnvResourceDetector, TelemetryResourceDetector},
    trace::{SdkTracer, SdkTracerProvider},
};

#[derive(Debug)]
pub(super) struct Telemetry(SdkTracerProvider);

impl Telemetry {
    pub(super) fn from_environment() -> Option<Self> {
        let configured = ["OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "OTEL_EXPORTER_OTLP_ENDPOINT"]
            .into_iter()
            .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()));
        if !configured
            || env::var("OTEL_SDK_DISABLED").is_ok_and(|value| value.eq_ignore_ascii_case("true"))
            || env::var("OTEL_TRACES_EXPORTER")
                .is_ok_and(|value| value.eq_ignore_ascii_case("none"))
        {
            return None;
        }
        // The blocking HTTP exporter must be constructed outside a Tokio
        // runtime. Session::init is also called from the NAPI runtime.
        let built = std::thread::Builder::new()
            .spawn(Self::build)
            .ok()
            .and_then(|thread| thread.join().ok())
            .and_then(Result::ok);
        if built.is_none() {
            tracing::warn!("could not initialize remote cache OpenTelemetry export");
        }
        built
    }

    fn build() -> Result<Self, opentelemetry_otlp::ExporterBuildError> {
        let builder = SpanExporter::builder().with_http();
        let builder = if ["OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "OTEL_EXPORTER_OTLP_PROTOCOL"]
            .into_iter()
            .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()))
        {
            builder
        } else {
            builder.with_protocol(Protocol::HttpJson)
        };
        let exporter = builder.build()?;
        let mut resource = Resource::builder_empty()
            .with_service_name("vite-task")
            .with_detector(Box::new(TelemetryResourceDetector))
            .with_detector(Box::new(EnvResourceDetector::new()));
        if let Ok(name) = env::var("OTEL_SERVICE_NAME")
            && !name.is_empty()
        {
            resource = resource.with_service_name(name);
        }
        Ok(Self(
            SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(resource.build())
                .build(),
        ))
    }

    pub(super) fn tracer(&self) -> SdkTracer {
        self.0.tracer("vite-task.remote-cache")
    }

    pub(super) async fn shutdown(self) {
        // The batch processor's worker owns the blocking HTTP exporter.
        // Await its final drain before the CLI can call process.exit.
        let result = tokio::task::spawn_blocking(move || self.0.shutdown()).await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!("could not flush remote cache OpenTelemetry export");
        }
    }
}
