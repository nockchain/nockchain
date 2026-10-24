/// Builds the OTLP gRPC subscriber, returning invalid exporter configuration to the caller.
pub fn init_tracing() -> Result<impl tracing::Subscriber, opentelemetry_otlp::ExporterBuildError> {
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
        .build()?;

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

    // Suppress noisy opentelemetry_sdk export errors (e.g. "tcp connect error")
    // unless the user explicitly enables them via RUST_LOG.
    let base_directives = "opentelemetry_sdk=off";
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(user) => tracing_subscriber::EnvFilter::new(format!("{},{}", base_directives, user)),
        Err(_) => tracing_subscriber::EnvFilter::new(base_directives),
    };
    let subscriber = tracing_subscriber::Registry::default()
        .with(env_filter)
        .with(fmt_layer)
        .with(telemetry);
    Ok(subscriber)
}
