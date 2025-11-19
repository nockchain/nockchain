use std::path::PathBuf;

use chrono;
use clap::{arg, command, Args, ColorChoice, Parser, ValueEnum};
use nockvm::jets::hot::HotEntry;
use nockvm::noun::Atom;
use nockvm::trace::{
    IntervalFilter, JsonBackend, KeywordFilter, TraceBackend, TraceFilter, TraceInfo,
    TracingBackend,
};
use tokio::fs;
use tracing::{debug, info, Level, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

use crate::export::ExportedState;
use crate::kernel::form::Kernel;
use crate::noun::slab::{Jammer, NounSlab};
use crate::save::SaveableCheckpoint;
use crate::utils::error::{CrownError, ExternalError};
use crate::{default_data_dir, AtomExt, NockApp};

pub const DEFAULT_SAVE_INTERVAL: u64 = 120000;
const DEFAULT_SAVE_INTERVAL_STR: &str = "120000";
const DEFAULT_LOG_FILTER: &str = "info";

#[derive(Debug, Clone, ValueEnum)]
pub enum NockStackSize {
    Tiny,
    Small,
    Normal,
    Medium,
    Large,
    Huge,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TraceMode {
    Json,
    Tracing,
}

/// Trace options for NockApp
#[derive(Args, Clone, Debug, Default)]
pub struct TraceOpts {
    /// You don't really need this, but it is here in case a new tracing backend is added or you want to use JSON tracing.
    /// We strongly recommend using Tracy
    #[arg(long = "trace", help = "Make a Sword trace in json or tracing mode")]
    pub mode: Option<TraceMode>,

    #[arg(long, requires = "mode")]
    pub keyword_filter: Option<String>,

    #[arg(long, requires = "mode")]
    pub interval_filter: Option<usize>,
}

impl From<TraceOpts> for Option<TraceInfo> {
    fn from(trace_opts: TraceOpts) -> Self {
        let keyword_filter = trace_opts
            .keyword_filter
            .map(|v| v.split(",").map(String::from).collect::<Vec<String>>())
            .map(|keywords| KeywordFilter { keywords });
        let interval_filter = trace_opts
            .interval_filter
            .map(|interval| IntervalFilter { interval, cnt: 0 });

        let filter = match (keyword_filter, interval_filter) {
            (Some(a), Some(b)) => Some(a.or(b).boxed()),
            (Some(a), _) => Some(a.boxed()),
            (_, Some(b)) => Some(b.boxed()),
            (None, None) => None,
        };

        trace_opts
            .mode
            .map(|mode| match mode {
                TraceMode::Json => {
                    let file = std::fs::File::create("trace.json")
                        .expect("Cannot create trace file trace.json");
                    let pid = std::process::id();
                    let process_start = std::time::Instant::now();

                    Box::new(JsonBackend {
                        file,
                        pid,
                        process_start,
                    }) as Box<dyn TraceBackend>
                }
                TraceMode::Tracing => Box::new(TracingBackend::new()),
            })
            .map(|backend| TraceInfo { backend, filter })
    }
}

#[derive(Parser, Debug, Clone)]
#[command(about = "boot a nockapp", author, version, color = ColorChoice::Auto)]
pub struct Cli {
    #[arg(
        long,
        help = "Start with a new data directory, removing any existing data",
        default_value = "false"
    )]
    pub new: bool,

    #[command(flatten)]
    pub trace_opts: TraceOpts,

    #[arg(
        long,
        help = "Set the save interval for checkpoints (in ms). Use 'none' or '0' to disable periodic saves.",
        default_value = DEFAULT_SAVE_INTERVAL_STR,
        value_parser = parse_save_interval
    )]
    pub save_interval: Option<u64>,

    #[arg(long, help = "Control colored output", value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,

    #[arg(
        long,
        help = "Path to a jam file containing existing kernel state. Supports both JammedCheckpoint and ExportedState formats."
    )]
    pub state_jam: Option<String>,

    #[arg(
        long,
        help = "Path to export the kernel state as a jam file in the ExportedState format."
    )]
    pub export_state_jam: Option<String>,

    #[arg(
        long,
        help = "Nock stack size to use",
        value_enum,
        default_value_t = NockStackSize::Normal
    )]
    pub stack_size: NockStackSize,
}

impl Cli {
    fn normalized_save_interval(&self) -> Option<u64> {
        self.save_interval
            .and_then(|value| if value == 0 { None } else { Some(value) })
    }
}

fn parse_save_interval(input: &str) -> Result<u64, String> {
    let trimmed = input.trim();

    if trimmed.eq_ignore_ascii_case("none") {
        Ok(0)
    } else {
        let value = trimmed
            .parse::<u64>()
            .map_err(|e| format!("Invalid save interval '{trimmed}': {e}"))?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::parse_save_interval;

    #[test]
    fn parse_save_interval_none_variants() {
        assert_eq!(parse_save_interval("none").unwrap(), 0);
        assert_eq!(parse_save_interval("NoNe").unwrap(), 0);
        assert_eq!(parse_save_interval("0").unwrap(), 0);
        assert_eq!(parse_save_interval(" 0 ").unwrap(), 0);
    }

    #[test]
    fn parse_save_interval_positive_values() {
        assert_eq!(parse_save_interval("1").unwrap(), 1);
        assert_eq!(parse_save_interval(" 120000 ").unwrap(), 120000);
    }

    #[test]
    fn parse_save_interval_rejects_invalid() {
        assert!(parse_save_interval("abc").is_err());
    }

    #[test]
    fn normalized_save_interval_filters_zero() {
        let mut cli = super::default_boot_cli(false);
        cli.save_interval = Some(0);
        assert_eq!(cli.normalized_save_interval(), None);

        cli.save_interval = Some(5000);
        assert_eq!(cli.normalized_save_interval(), Some(5000));
    }
}

/// Result of setting up a NockApp
pub enum SetupResult<J> {
    /// A fully initialized NockApp
    App(NockApp<J>),
    /// State was exported successfully
    ExportedState,
}

pub fn default_boot_cli(new: bool) -> Cli {
    Cli {
        save_interval: Some(DEFAULT_SAVE_INTERVAL),
        new,
        trace_opts: Default::default(),
        color: ColorChoice::Auto,
        state_jam: None,
        export_state_jam: None,
        stack_size: NockStackSize::Normal,
    }
}

/// A minimal event formatter for development mode
struct MinimalFormatter;

impl<S, N> FormatEvent<S, N> for MinimalFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let level = *event.metadata().level();
        let level_str = match level {
            Level::TRACE => "\x1B[36mT\x1B[0m",
            Level::DEBUG => "\x1B[34mD\x1B[0m",
            Level::INFO => "\x1B[32mI\x1B[0m",
            Level::WARN => "\x1B[33mW\x1B[0m",
            Level::ERROR => "\x1B[31mE\x1B[0m",
        };

        // Get level color code for potential use with slogger
        let level_color = match level {
            Level::TRACE => "\x1B[36m", // Cyan
            Level::DEBUG => "\x1B[34m", // Blue
            Level::INFO => "\x1B[32m",  // Green
            Level::WARN => "\x1B[33m",  // Yellow
            Level::ERROR => "\x1B[31m", // Red
        };

        write!(writer, "{} ", level_str)?;

        // simple, shorter timestamp (HH:mm:ss)
        let now = chrono::Local::now();
        let time_str = now.format("%H:%M:%S").to_string();
        write!(writer, "\x1B[38;5;246m({time_str})\x1B[0m ")?;

        let target = event.metadata().target();

        // Special handling for slogger
        if target == "slogger" {
            // For slogger, omit the target prefix and color the message with the log level color
            // this mimics the behavior of slogging in urbit
            write!(writer, "{}", level_color)?;
            ctx.field_format().format_fields(writer.by_ref(), event)?;
            write!(writer, "\x1B[0m")?;

            return writeln!(writer);
        }

        let simplified_target = if target.contains("::") {
            // Just take the last component of the module path
            let parts: Vec<&str> = target.split("::").collect();
            if parts.len() > 1 {
                // If we have a structure like "a::b::c::d", just take "c::d"
                // but prefix it with the first two characters of the first part
                // i.e, nockapp::kernel::boot -> [cr] kernel::boot
                if parts.len() > 2 {
                    format!(
                        "[{}] {}::{}",
                        parts[0].chars().take(2).collect::<String>(),
                        parts[parts.len() - 2],
                        parts[parts.len() - 1]
                    )
                } else {
                    parts
                        .last()
                        .unwrap_or_else(|| {
                            panic!(
                                "Panicked at {}:{} (git sha: {:?})",
                                file!(),
                                line!(),
                                option_env!("GIT_SHA")
                            )
                        })
                        .to_string()
                }
            } else {
                target.to_string()
            }
        } else {
            target.to_string()
        };

        // Write the simplified target in grey and italics
        write!(writer, "\x1B[3;90m{}\x1B[0m: ", simplified_target)?;

        // Write the fields (the actual log message)
        ctx.field_format().format_fields(writer.by_ref(), event)?;

        writeln!(writer)
    }
}

fn init_with_default_filter<T: Subscriber + Send + Sync + for<'a> LookupSpan<'a>>(reg: T) {
    let filter = EnvFilter::new(
        std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_LOG_FILTER.to_string()),
    );

    let reg = reg.with(filter);

    #[cfg(feature = "tracing-tracy")]
    if std::env::var("TRACY_DISABLE").is_err() {
        let tracy = tracing_tracy::TracyLayer::default();
        let only_nockcode = std::env::var("TRACY_ONLY_NOCKCODE").is_ok();
        if only_nockcode {
            let nockcode_filter =
                tracing_subscriber::filter::filter_fn(move |meta| meta.target() == "nockcode");
            reg.with(tracy.with_filter(nockcode_filter)).init();
        } else {
            reg.with(tracy).init();
        }
        debug!("Tracy tracing is enabled");
        return;
    } else {
        debug!("Tracy tracing is disabled");
    }
    reg.init();
}

/// Initialize tracing with appropriate configuration based on CLI arguments.
pub fn init_default_tracing(cli: &Cli) {
    let use_ansi = cli.color == ColorChoice::Auto || cli.color == ColorChoice::Always;

    // Build and initialize the subscriber
    // If RUST_LOG is set and MINIMAL_LOG_FORMAT is unset, we will do production-grade logging.
    // Otherwise we will do more minimal logging suitable for an interactive terminal.
    if std::env::var("MINIMAL_LOG_FORMAT").is_ok() || std::env::var("RUST_LOG").is_err() {
        let fmt_layer = fmt::layer()
            .with_ansi(use_ansi)
            .event_format(MinimalFormatter);

        init_with_default_filter(tracing_subscriber::registry().with(fmt_layer));
    } else {
        init_with_default_filter(
            tracing_subscriber::registry().with(
                fmt::layer()
                    .with_ansi(use_ansi)
                    .with_target(true)
                    .with_level(true),
            ),
        );
    }
}

pub async fn setup<J: Jammer + Send + 'static>(
    jam: &[u8],
    cli: Cli,
    hot_state: &[HotEntry],
    name: &str,
    data_dir: Option<PathBuf>,
) -> Result<NockApp<J>, Box<dyn std::error::Error>> {
    let result = setup_(jam, cli, hot_state, name, data_dir).await?;
    match result {
        SetupResult::App(app) => Ok(app),
        SetupResult::ExportedState => {
            info!("Exiting after successful state export");
            std::process::exit(0);
        }
    }
}

pub async fn setup_<J: Jammer + Send + 'static>(
    jam: &[u8],
    cli: Cli,
    hot_state: &[HotEntry],
    name: &str,
    data_dir: Option<PathBuf>,
) -> Result<SetupResult<J>, Box<dyn std::error::Error>> {
    let nock_test_jets_env = std::env::var("NOCK_TEST_JETS").unwrap_or_default();
    let test_jets = parse_test_jets(nock_test_jets_env.as_str());
    let data_dir = if let Some(data_path) = data_dir.clone() {
        data_path.join(name)
    } else {
        default_data_dir(name)
    };
    let pma_dir = data_dir.join("pma");
    let jams_dir = data_dir.join("checkpoints");

    if !jams_dir.exists() {
        std::fs::create_dir_all(&jams_dir)?;
        debug!("Created jams directory: {:?}", jams_dir);
    }

    if pma_dir.exists() {
        std::fs::remove_dir_all(&pma_dir)?;
        debug!("Deleted existing pma directory: {:?}", pma_dir);
    }

    if cli.new && jams_dir.exists() {
        std::fs::remove_dir_all(&jams_dir)?;
        debug!("Deleted existing checkpoint directory: {:?}", jams_dir);
    }

    info!("kernel: starting");
    debug!("kernel: pma directory: {:?}", pma_dir);
    debug!("kernel: snapshots directory: {:?}", jams_dir);
    info!("NockApp boot cli: {:?}", cli);
    let save_interval = cli
        .normalized_save_interval()
        .map(std::time::Duration::from_millis);

    let kernel_f = async |checkpoint| {
        let kernel: Kernel<SaveableCheckpoint> = match cli.stack_size {
            NockStackSize::Tiny => {
                Kernel::load_with_hot_state_tiny(
                    jam, checkpoint, hot_state, test_jets, cli.trace_opts,
                )
                .await?
            }
            NockStackSize::Small => {
                Kernel::load_with_hot_state_small(
                    jam, checkpoint, hot_state, test_jets, cli.trace_opts,
                )
                .await?
            }
            NockStackSize::Normal => {
                Kernel::load_with_hot_state(jam, checkpoint, hot_state, test_jets, cli.trace_opts)
                    .await?
            }
            NockStackSize::Medium => {
                Kernel::load_with_hot_state_medium(
                    jam, checkpoint, hot_state, test_jets, cli.trace_opts,
                )
                .await?
            }
            NockStackSize::Large => {
                Kernel::load_with_hot_state_large(
                    jam, checkpoint, hot_state, test_jets, cli.trace_opts,
                )
                .await?
            }
            NockStackSize::Huge => {
                Kernel::load_with_hot_state_huge(
                    jam, checkpoint, hot_state, test_jets, cli.trace_opts,
                )
                .await?
            }
        };
        let res: Result<Kernel<SaveableCheckpoint>, CrownError<ExternalError>> = Ok(kernel);
        res
    };

    let app: NockApp<J> = NockApp::new(kernel_f, &jams_dir, save_interval).await?;

    if let Some(export_path) = cli.export_state_jam.clone() {
        export_kernel_state(&app.kernel, &export_path).await?;
        return Ok(SetupResult::ExportedState);
    }

    if let Some(import_path) = cli.state_jam.clone() {
        import_kernel_state(&app.kernel, &import_path).await?;
    }

    Ok(SetupResult::App(app))
}

/// Exports the kernel state to a jam file at the specified path
async fn export_kernel_state<C>(
    kernel: &Kernel<C>,
    export_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let kernel_state = kernel.export().await?;
    let exported_state = ExportedState::from_loadstate(kernel_state);
    let state_bytes = exported_state.encode()?;
    fs::write(export_path, state_bytes).await?;
    info!("Successfully exported kernel state to: {:?}", export_path);
    Ok(())
}

/// Imports the kernel state from a jam file at the specified path
async fn import_kernel_state<C>(
    kernel: &Kernel<C>,
    import_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_bytes = fs::read(import_path).await?;
    let exported_state = ExportedState::decode(&state_bytes)?;
    let kernel_state = exported_state.to_loadstate()?;
    kernel.import(kernel_state).await?;
    info!("Successfully imported kernel state from: {:?}", import_path);
    Ok(())
}

pub fn parse_test_jets(jets: &str) -> Vec<NounSlab> {
    let mut test_jets = Vec::new();
    for jet in jets.split(',') {
        if jet.is_empty() {
            continue;
        }
        let mut slab = NounSlab::new();
        let mut path = nockvm::noun::D(0);
        for el in jet.split('/') {
            let ver_split: Vec<&str> = el.split('.').collect();
            if ver_split.len() == 2 {
                let sym_atom = Atom::from_value(&mut slab, ver_split[0])
                    .expect("Could not construct symbol atom")
                    .as_noun();
                let ver_atom = Atom::from_value(
                    &mut slab,
                    u64::from_str_radix(ver_split[1], 10)
                        .expect("Could not parse cold path version"),
                )
                .expect("Could not construct version atom")
                .as_noun();
                let path_el = nockvm::noun::T(&mut slab, &[sym_atom, ver_atom]);
                path = nockvm::noun::T(&mut slab, &[path_el, path]);
            } else if el.is_empty() {
                continue;
            } else {
                let el_atom = Atom::from_value(&mut slab, el)
                    .expect("Could not construct element atom")
                    .as_noun();
                path = nockvm::noun::T(&mut slab, &[el_atom, path]);
            }
        }
        slab.set_root(path);
        test_jets.push(slab);
    }
    test_jets
}
