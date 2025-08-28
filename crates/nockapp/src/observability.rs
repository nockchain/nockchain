pub fn init_tracing() -> Result<impl tracing::Subscriber, opentelemetry_sdk::trace::TraceError> {
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_otlp::WithExportConfig;
    use opentelemetry_sdk::trace::Sampler;
    use tracing_subscriber::layer::SubscriberExt;

    // Datadog agent OTLP endpoint configuration
    let dd_agent_host = std::env::var("DD_AGENT_HOST").unwrap_or("localhost".to_owned());
    let dd_trace_port = std::env::var("DD_OTLP_GRPC_PORT").unwrap_or("4317".to_owned());
    let endpoint = format!("http://{}:{}", dd_agent_host, dd_trace_port);

    eprintln!("Datadog APM endpoint (OTLP gRPC): {}", endpoint);

    // Service information
    let service_name = std::env::var("DD_SERVICE").unwrap_or("nockapp".to_owned());
    let service_version = std::env::var("DD_VERSION").unwrap_or("0.1.0".to_owned());
    let environment = std::env::var("DD_ENV").unwrap_or("development".to_owned());

    // Resource attributes that Datadog requires
    let resource = opentelemetry_sdk::Resource::builder_empty()
        .with_attribute(opentelemetry::KeyValue::new(
            "service.name",
            service_name.clone(),
        ))
        .with_attribute(opentelemetry::KeyValue::new(
            "service.version", service_version,
        ))
        .with_attribute(opentelemetry::KeyValue::new(
            "deployment.environment", environment,
        ))
        .build();

    // Create OTLP exporter configured for Datadog
    let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .with_timeout(std::time::Duration::from_secs(30))
        .with_protocol(opentelemetry_otlp::Protocol::Grpc)
        .build()
        .unwrap_or_else(|_| {
            panic!(
                "Panicked at {}:{} (git sha: {:?})",
                file!(),
                line!(),
                option_env!("GIT_SHA")
            )
        });

    // Configure sampling rate - 0.1 means sample ~10% of traces
    let sampling_ratio = std::env::var("OTEL_TRACES_SAMPLE_RATE")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(1.0);

    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_batch_exporter(otlp_exporter)
        .with_resource(resource)
        // Add the probability sampler here
        .with_sampler(Sampler::TraceIdRatioBased(sampling_ratio))
        .build();

    let use_ansi = std::env::var("DD_ENV").is_err();
    let tracer = provider.tracer(service_name);
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(true)
        .with_line_number(true)
        // .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
        .with_ansi(use_ansi);

    let subscriber = tracing_subscriber::Registry::default()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(fmt_layer)
        .with(telemetry);
    Ok(subscriber)
}

// pub fn init_tracing() -> Result<impl tracing::Subscriber, opentelemetry::trace::TraceError> {
//     use opentelemetry::trace::TracerProvider;
//     use opentelemetry_otlp::WithExportConfig;
//     use opentelemetry_sdk::trace::{Sampler, SamplingResult, ShouldSample};
//     use tracing_subscriber::layer::SubscriberExt;
//     use opentelemetry::{Context, Key, KeyValue};
//     use opentelemetry_sdk::trace::TraceState;
//     use std::sync::Arc;

//     // Create a custom sampler that handles ERROR level spans differently
//     #[derive(Debug)]
//     struct ErrorAwareSampler {
//         base_sampler: Sampler,
//         error_level_key: Key,
//     }

//     impl ShouldSample for ErrorAwareSampler {
//         fn should_sample(
//             &self,
//             parent_context: Option<&Context>,
//             trace_id: opentelemetry::trace::TraceId,
//             name: &str,
//             span_kind: &opentelemetry::trace::SpanKind,
//             attributes: &[KeyValue],
//             links: &[opentelemetry::trace::Link],
//         ) -> SamplingResult {
//             // Check if this span has the ERROR level attribute
//             for attr in attributes {
//                 if attr.key == self.error_level_key && attr.value.as_str() == Some("ERROR") {
//                     // Always sample error spans
//                     return SamplingResult {
//                         decision: opentelemetry::trace::SamplingDecision::RecordAndSample,
//                         attributes: Vec::new(),
//                         trace_state: TraceState::default(),
//                     };
//                 }
//             }

//             // For non-error spans, use the base sampler
//             self.base_sampler.should_sample(
//                 parent_context,
//                 trace_id,
//                 name,
//                 span_kind,
//                 attributes,
//                 links,
//             )
//         }
//     }

//     let endpoint = std::env::var(JAEGER_ENDPOINT_ENV).unwrap_or("http://localhost:4317".to_owned());
//     eprintln!("OTLP gRPC endpoint: {}", endpoint);
//     let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
//         .with_tonic()
//         .with_endpoint(endpoint)
//         .build()
//         .unwrap();

//     // Configure sampling ratio - 0.1 means sample ~10% of traces
//     let sampling_ratio = std::env::var("OTEL_TRACES_SAMPLE_RATE")
//         .ok()
//         .and_then(|v| v.parse::<f64>().ok())
//         .unwrap_or(0.1);

//     // Create our custom error-aware sampler
//     let error_aware_sampler = ErrorAwareSampler {
//         base_sampler: Sampler::TraceIdRatioBased(sampling_ratio),
//         error_level_key: Key::from_static_str("log.level"),
//     };

//     let provider = opentelemetry_sdk::trace::TracerProvider::builder()
//         .with_batch_exporter(otlp_exporter, opentelemetry_sdk::runtime::Tokio)
//         .with_sampler(error_aware_sampler)
//         .build();

//     // Add level to spans so sampler can use it
//     let env_filter = tracing_subscriber::EnvFilter::from_default_env();
//     let fmt_layer = tracing_subscriber::fmt::layer()
//         .with_target(true)
//         .with_thread_ids(true)
//         .with_line_number(true);

//     let tracer = provider.tracer("nockapp");
//     let telemetry = tracing_opentelemetry::layer()
//         .with_tracer(tracer)
//         // This ensures log level gets propagated to span attributes
//         .with_tracked_inactivity(true);

//     let subscriber = tracing_subscriber::Registry::default()
//         .with(env_filter)
//         .with(fmt_layer)
//         .with(telemetry);
//     Ok(subscriber)
// }

// pub fn init_tracing() -> Result<impl tracing::Subscriber, opentelemetry::trace::TraceError> {
//     use opentelemetry::trace::TracerProvider;
//     use opentelemetry_otlp::WithExportConfig;
//     use opentelemetry_sdk::trace::{Sampler, SamplingResult, ShouldSample};
//     use tracing_subscriber::layer::SubscriberExt;
//     use opentelemetry::{Context, Key, KeyValue};
//     use opentelemetry_sdk::trace::TraceState;
//     use std::sync::Arc;

//     // Create a custom sampler that handles ERROR level spans differently
//     #[derive(Debug)]
//     struct ErrorAwareSampler {
//         base_sampler: Sampler,
//         error_level_key: Key,
//     }

//     impl ShouldSample for ErrorAwareSampler {
//         fn should_sample(
//             &self,
//             parent_context: Option<&Context>,
//             trace_id: opentelemetry::trace::TraceId,
//             name: &str,
//             span_kind: &opentelemetry::trace::SpanKind,
//             attributes: &[KeyValue],
//             links: &[opentelemetry::trace::Link],
//         ) -> SamplingResult {
//             // Check if this span has the ERROR level attribute
//             for attr in attributes {
//                 if attr.key == self.error_level_key && attr.value.as_str() == Some("ERROR") {
//                     // Always sample error spans
//                     return SamplingResult {
//                         decision: opentelemetry::trace::SamplingDecision::RecordAndSample,
//                         attributes: Vec::new(),
//                         trace_state: TraceState::default(),
//                     };
//                 }
//             }

//             // For non-error spans, use the base sampler
//             self.base_sampler.should_sample(
//                 parent_context,
//                 trace_id,
//                 name,
//                 span_kind,
//                 attributes,
//                 links,
//             )
//         }
//     }

//     let endpoint = std::env::var(JAEGER_ENDPOINT_ENV).unwrap_or("http://localhost:4317".to_owned());
//     eprintln!("OTLP gRPC endpoint: {}", endpoint);
//     let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
//         .with_tonic()
//         .with_endpoint(endpoint)
//         .build()
//         .unwrap();

//     // Configure sampling ratio - 0.1 means sample ~10% of traces
//     let sampling_ratio = std::env::var("OTEL_TRACES_SAMPLE_RATE")
//         .ok()
//         .and_then(|v| v.parse::<f64>().ok())
//         .unwrap_or(0.1);

//     // Create our custom error-aware sampler
//     let error_aware_sampler = ErrorAwareSampler {
//         base_sampler: Sampler::TraceIdRatioBased(sampling_ratio),
//         error_level_key: Key::from_static_str("log.level"),
//     };

//     let provider = opentelemetry_sdk::trace::TracerProvider::builder()
//         .with_batch_exporter(otlp_exporter, opentelemetry_sdk::runtime::Tokio)
//         .with_sampler(error_aware_sampler)
//         .build();

//     // Add level to spans so sampler can use it
//     let env_filter = tracing_subscriber::EnvFilter::from_default_env();
//     let fmt_layer = tracing_subscriber::fmt::layer()
//         .with_target(true)
//         .with_thread_ids(true)
//         .with_line_number(true);

//     let tracer = provider.tracer("nockapp");
//     let telemetry = tracing_opentelemetry::layer()
//         .with_tracer(tracer)
//         // This ensures log level gets propagated to span attributes
//         .with_tracked_inactivity(true);

//     let subscriber = tracing_subscriber::Registry::default()
//         .with(env_filter)
//         .with(fmt_layer)
//         .with(telemetry);
//     Ok(subscriber)
// }

// pub fn init_tracing() -> Result<impl tracing::Subscriber, opentelemetry::trace::TraceError> {
//     use opentelemetry::trace::TracerProvider;
//     use opentelemetry_otlp::WithExportConfig;
//     use tracing_subscriber::layer::SubscriberExt;

//     let endpoint = std::env::var(JAEGER_ENDPOINT_ENV).unwrap_or("http://localhost:4317".to_owned());
//     eprintln!("OTLP gRPC endpoint: {}", endpoint);
//     let otlp_exporter = opentelemetry_otlp::SpanExporter::builder()
//         .with_tonic()
//         .with_endpoint(endpoint)
//         .build()
//         .unwrap();

//     let provider = opentelemetry_sdk::trace::TracerProvider::builder()
//         .with_batch_exporter(otlp_exporter, opentelemetry_sdk::runtime::Tokio)
//         .build();

//     let tracer = provider.tracer("nockapp");
//     let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);
//     let fmt_layer = tracing_subscriber::fmt::layer()
//         .with_target(true)
//         // .with_span_events(tracing_subscriber::fmt::format::FmtSpan::ENTER)
//         .with_thread_ids(true)
//         .with_line_number(true);

//     let subscriber = tracing_subscriber::Registry::default()
//         .with(tracing_subscriber::EnvFilter::from_default_env())
//         .with(fmt_layer)
//         .with(telemetry);
//     Ok(subscriber)
// }
