use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context as _, Result};
use boa_engine::builtins::promise::PromiseState;
use boa_engine::property::Attribute;
use boa_engine::{
    js_string, Context, JsArgs, JsNativeError, JsResult, JsValue, NativeFunction, Source,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::backend::Backend;
use crate::catalog::{catalog, OutputFormat};

pub const DEFAULT_MAX_CODE_BYTES: usize = 32 * 1024;
pub const DEFAULT_MAX_RESULT_BYTES: usize = 1024 * 1024;
pub const DEFAULT_MAX_CALLS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum SandboxKind {
    Search,
    Execute,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestOptions {
    operation: String,
    #[serde(default = "empty_object")]
    input: Value,
    #[serde(default)]
    format: OutputFormat,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExplainOptions {
    operation: String,
    #[serde(default = "empty_object")]
    input: Value,
}

fn empty_object() -> Value {
    json!({})
}

pub fn run_code(
    kind: SandboxKind,
    backend: Backend,
    code: &str,
    max_calls: usize,
    max_result_bytes: usize,
) -> Result<Value> {
    if code.len() > DEFAULT_MAX_CODE_BYTES {
        bail!(
            "code is {} bytes; maximum is {} bytes",
            code.len(),
            DEFAULT_MAX_CODE_BYTES
        );
    }
    let spec = catalog(backend.mode)?;
    let calls = Arc::new(AtomicUsize::new(0));
    let mut context = Context::default();
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(250_000);
    context.runtime_limits_mut().set_recursion_limit(256);
    context
        .runtime_limits_mut()
        .set_stack_size_limit(512 * 1024);

    let js_spec = boa_result(
        JsValue::from_json(&spec, &mut context),
        "convert the query catalog to JavaScript",
    )?;
    boa_result(
        context.register_global_property(
            js_string!("__nockchain_spec"),
            js_spec,
            Attribute::READONLY,
        ),
        "register the query catalog",
    )?;

    if kind == SandboxKind::Execute {
        register_request(&mut context, backend.clone(), Arc::clone(&calls), max_calls)?;
        register_explain(&mut context, backend)?;
        boa_result(
            context.eval(Source::from_bytes(
                br#"
                globalThis.codemode = Object.freeze({
                    spec: () => JSON.parse(JSON.stringify(__nockchain_spec)),
                    request: async (options) => __nockchain_request(options),
                    explain: (options) => __nockchain_explain(options),
                });
            "#,
            )),
            "initialize the execute sandbox",
        )?;
    } else {
        boa_result(
            context.eval(Source::from_bytes(
                br#"
                globalThis.codemode = Object.freeze({
                    spec: () => JSON.parse(JSON.stringify(__nockchain_spec)),
                });
            "#,
            )),
            "initialize the search sandbox",
        )?;
    }

    let script = format!("const __nockchain_result = ({code})(); __nockchain_result;");
    let value = context
        .eval(Source::from_bytes(script.as_bytes()))
        .map_err(|error| anyhow!("JavaScript evaluation failed: {error}"))?;
    context
        .run_jobs()
        .map_err(|error| anyhow!("JavaScript job failed: {error}"))?;
    let value = settle(value, &mut context)?;
    let result = boa_result(
        value.to_json(&mut context),
        "convert the JavaScript result to JSON",
    )?
    .unwrap_or(Value::Null);
    let result_size = serde_json::to_vec(&result)?.len();
    if result_size > max_result_bytes {
        bail!(
            "JavaScript result is {result_size} bytes; maximum is {max_result_bytes} bytes. Filter or project the result in code."
        );
    }
    Ok(result)
}

fn register_request(
    context: &mut Context,
    backend: Backend,
    calls: Arc<AtomicUsize>,
    max_calls: usize,
) -> Result<()> {
    // SAFETY: the closure captures only Rust-owned Backend/Arc values and no Boa GC-managed
    // values. Neither capture can contain or outlive a JavaScript heap reference.
    let function = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let call_number = calls.fetch_add(1, Ordering::Relaxed) + 1;
            if call_number > max_calls {
                return Err(js_error(format!(
                    "code exceeded the maximum of {max_calls} backend calls"
                )));
            }
            let input = args
                .get_or_undefined(0)
                .to_json(context)?
                .context("codemode.request options are undefined")
                .map_err(|error| js_error(error.to_string()))?;
            let options: RequestOptions = serde_json::from_value(input)
                .map_err(|error| js_error(format!("invalid codemode.request options: {error}")))?;
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(backend.request(&options.operation, options.input, options.format))
            })
            .map_err(|error| js_error(format!("{error:#}")))?;
            JsValue::from_json(&result, context)
        })
    };
    boa_result(
        context.register_global_callable(js_string!("__nockchain_request"), 1, function),
        "register codemode.request",
    )?;
    Ok(())
}

fn register_explain(context: &mut Context, backend: Backend) -> Result<()> {
    // SAFETY: Backend is ordinary Rust-owned data and contains no Boa GC-managed references.
    let function = unsafe {
        NativeFunction::from_closure(move |_this, args, context| {
            let input = args
                .get_or_undefined(0)
                .to_json(context)?
                .context("codemode.explain options are undefined")
                .map_err(|error| js_error(error.to_string()))?;
            let options: ExplainOptions = serde_json::from_value(input)
                .map_err(|error| js_error(format!("invalid codemode.explain options: {error}")))?;
            let result = backend
                .explain(&options.operation, options.input)
                .map_err(|error| js_error(error.to_string()))?;
            JsValue::from_json(&result, context)
        })
    };
    boa_result(
        context.register_global_callable(js_string!("__nockchain_explain"), 1, function),
        "register codemode.explain",
    )?;
    Ok(())
}

fn settle(value: JsValue, context: &mut Context) -> Result<JsValue> {
    let Some(promise) = value.as_promise() else {
        return Ok(value);
    };
    match promise.state() {
        PromiseState::Fulfilled(value) => Ok(value),
        PromiseState::Rejected(reason) => {
            let message = reason
                .to_string(context)
                .map(|message| message.to_std_string_escaped())
                .unwrap_or_else(|_| "unknown rejection".to_string());
            bail!("JavaScript promise rejected: {message}")
        }
        PromiseState::Pending => bail!(
            "JavaScript promise remained pending; timers, fetch, imports, and external async APIs are unavailable"
        ),
    }
}

fn js_error(message: String) -> boa_engine::JsError {
    JsNativeError::error().with_message(message).into()
}

fn boa_result<T>(result: JsResult<T>, action: &str) -> Result<T> {
    result.map_err(|error| anyhow!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::catalog::ApiMode;

    fn backend(mode: ApiMode) -> Backend {
        Backend {
            mode,
            endpoint: mode.default_backend().to_string(),
            timeout: Duration::from_secs(1),
        }
    }

    #[test]
    fn search_filters_the_in_memory_catalog() {
        let result = run_code(
            SandboxKind::Search,
            backend(ApiMode::Public),
            "async () => codemode.spec().operations.filter(op => op.name.includes('block')).map(op => op.name)",
            DEFAULT_MAX_CALLS,
            DEFAULT_MAX_RESULT_BYTES,
        )
        .expect("search code");
        assert_eq!(
            result,
            json!(["get_blocks", "get_block_details", "get_transaction_block"])
        );
    }

    #[test]
    fn execute_explains_without_contacting_a_node() {
        let result = run_code(
            SandboxKind::Execute,
            backend(ApiMode::Public),
            "async () => codemode.explain({ operation: 'get_blocks', input: { page: { clientPageItemsLimit: 1 } } })",
            DEFAULT_MAX_CALLS,
            DEFAULT_MAX_RESULT_BYTES,
        )
        .expect("execute explain code");
        assert_eq!(
            result["grpc"]["fullMethod"],
            "/nockchain.public.v2.NockchainBlockService/GetBlocks"
        );
    }

    #[test]
    fn execute_cannot_explain_mutations() {
        let error = run_code(
            SandboxKind::Execute,
            backend(ApiMode::Public),
            "async () => codemode.explain({ operation: 'wallet_send_transaction', input: {} })",
            DEFAULT_MAX_CALLS,
            DEFAULT_MAX_RESULT_BYTES,
        )
        .expect_err("mutation must fail");
        assert!(error.to_string().contains("unknown or unavailable"));
    }

    #[test]
    fn loops_are_limited() {
        let error = run_code(
            SandboxKind::Search,
            backend(ApiMode::Public),
            "() => { while (true) {} }",
            DEFAULT_MAX_CALLS,
            DEFAULT_MAX_RESULT_BYTES,
        )
        .expect_err("infinite loop must fail");
        assert!(error.to_string().contains("loop iteration"));
    }
}
