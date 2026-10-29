//! HTTP client spans and W3C context propagation for cache operations.

use opentelemetry::{
    Context, KeyValue,
    global::BoxedTracer,
    propagation::TextMapPropagator as _,
    trace::{SpanKind, Status, TraceContextExt as _, Tracer as _},
};
use opentelemetry_http::HeaderInjector;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use reqwest::{Response, header::HeaderMap};
use url::Url;

use crate::Error;

/// Owns a span until the operation (including a streamed body) finishes.
#[derive(Debug)]
pub struct RequestSpan(Context);

impl RequestSpan {
    pub(super) fn start(
        tracer: Option<&BoxedTracer>,
        method: &'static str,
        route: &'static str,
        url: &Url,
        headers: &mut HeaderMap,
    ) -> Option<Self> {
        let tracer = tracer?;
        // Only transport metadata is recorded. Endpoint paths, queries,
        // credentials, cache keys and request/response bodies stay out.
        let mut attributes = vec![
            KeyValue::new("http.request.method", method),
            KeyValue::new("url.scheme", url.scheme().to_owned()),
        ];
        if let Some(host) = url.host_str() {
            attributes.push(KeyValue::new("server.address", host.to_owned()));
        }
        if let Some(port) = url.port_or_known_default() {
            attributes.push(KeyValue::new("server.port", i64::from(port)));
        }
        let span = tracer.build(
            tracer
                .span_builder(vt_str::format!("{method} /{route}").to_string())
                .with_kind(SpanKind::Client)
                .with_attributes(attributes),
        );
        let context = Context::current_with_span(span);
        TraceContextPropagator::new().inject_context(&context, &mut HeaderInjector(headers));
        Some(Self(context))
    }

    pub(super) fn response(&self, response: &Response) {
        self.0.span().set_attribute(KeyValue::new(
            "http.response.status_code",
            i64::from(response.status().as_u16()),
        ));
    }

    pub(super) fn error(&self, error: &Error) {
        // Error messages can include the server's response body. Export only
        // the error class, never the message or source chain.
        let kind = match error {
            Error::InvalidEndpoint(_) => "invalid_endpoint",
            Error::HttpClient(_) => "http_client",
            Error::ReadBlob(_) => "read_blob",
            Error::Auth(_) => "auth",
            Error::Network(_) => "network",
            Error::Status(status, _) => {
                self.0
                    .span()
                    .set_attribute(KeyValue::new("error.type", status.as_u16().to_string()));
                self.0.span().set_status(Status::error("HTTP request failed"));
                return;
            }
            Error::MalformedResponse(_) => "malformed_response",
        };
        self.0.span().set_attribute(KeyValue::new("error.type", kind));
        self.0.span().set_status(Status::error(kind));
    }

    pub(super) fn finish<T>(span: Option<Self>, result: Result<T, Error>) -> Result<T, Error> {
        if let (Some(span), Err(error)) = (&span, &result) {
            span.error(error);
        }
        drop(span);
        result
    }
}

impl Drop for RequestSpan {
    fn drop(&mut self) {
        self.0.span().end();
    }
}
