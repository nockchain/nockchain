//! Gemma 4 31B checkpoint validation and dense peak operand mapping.
//!
//! Nockchain consensus admits arbitrary INT7 matrices inside the Pearl production
//! envelope. Model identity is therefore miner policy, not a consensus input. This
//! module validates the selected Pearl checkpoint and maps its mineable MLP operands
//! into the existing fixed peak geometry without changing the `%ai-pow` statement.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::PEAK_PRODUCTION_PARAMS;

/// Exact language model architecture supported by this target profile.
pub const GEMMA4_ARCHITECTURE: &str = "Gemma4ForConditionalGeneration";
/// Exact top-level model type supported by this target profile.
pub const GEMMA4_MODEL_TYPE: &str = "gemma4";
/// Exact text model type supported by this target profile.
pub const GEMMA4_TEXT_MODEL_TYPE: &str = "gemma4_text";
/// Number of decoder layers in the selected checkpoint.
pub const GEMMA4_LAYERS: usize = 60;
/// Input width of each mineable Gemma 4 MLP projection.
pub const GEMMA4_HIDDEN_SIZE: usize = 5_376;
/// Output width of each mineable Gemma 4 MLP projection.
pub const GEMMA4_INTERMEDIATE_SIZE: usize = 21_504;
/// Maximum logical token rows carried by one fixed peak work matrix.
pub const GEMMA4_MAX_TOKENS: usize = PEAK_PRODUCTION_PARAMS.m as usize;
/// Physical common dimension required by the production peak CUDA kernel.
pub const GEMMA4_PEAK_K: usize = PEAK_PRODUCTION_PARAMS.k as usize;
/// Physical output width required by the production peak CUDA kernel.
pub const GEMMA4_PEAK_N: usize = PEAK_PRODUCTION_PARAMS.n as usize;

const CONFIG_FILE: &str = "config.json";
const WEIGHTS_FILE: &str = "model.safetensors";
const MAX_SAFETENSORS_HEADER_BYTES: u64 = 16 * 1024 * 1024;
const REFERENCE_QUANTIZATION_VERSION: &str = "0.15.0.1";
const FULL_ATTENTION_INPUT_SIZE: u64 = 16_384;
const SLIDING_ATTENTION_INPUT_SIZE: u64 = 8_192;

/// Mineable INT7 MLP projection in the selected Gemma 4 checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gemma4MlpProjection {
    Gate,
    Up,
}

impl Gemma4MlpProjection {
    const fn tensor_component(self) -> &'static str {
        match self {
            Self::Gate => "gate_proj",
            Self::Up => "up_proj",
        }
    }

    /// Return the checkpoint tensor name for one decoder layer.
    pub fn tensor_name(self, layer: usize) -> Result<String, Gemma4TargetError> {
        validate_layer(layer)?;
        Ok(format!(
            "model.language_model.layers.{layer}.mlp.{}.weight",
            self.tensor_component()
        ))
    }
}

#[derive(Clone, Copy, Debug)]
struct TensorSlice {
    absolute_offset: u64,
    byte_len: u64,
}

#[derive(Clone, Copy, Debug)]
struct LayerTensorSlices {
    gate: TensorSlice,
    up: TensorSlice,
}

impl LayerTensorSlices {
    const fn get(self, projection: Gemma4MlpProjection) -> TensorSlice {
        match projection {
            Gemma4MlpProjection::Gate => self.gate,
            Gemma4MlpProjection::Up => self.up,
        }
    }
}

/// Validated metadata for the selected Pearl Gemma 4 31B checkpoint.
///
/// Opening a checkpoint reads only `config.json` and the bounded safetensors JSON
/// header. Weight bytes remain on disk until [`Self::load_peak_b_col_major`] is
/// called.
#[derive(Debug)]
pub struct Gemma4Checkpoint {
    root: PathBuf,
    weights_path: PathBuf,
    safetensors_layout_digest: [u8; 32],
    weights_file_len: u64,
    layers: Vec<LayerTensorSlices>,
}

impl Gemma4Checkpoint {
    /// Validate a checkpoint directory against the fixed Gemma 4 target profile.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, Gemma4TargetError> {
        let root = root.as_ref().to_path_buf();
        let config_path = root.join(CONFIG_FILE);
        let config_bytes = read_file(&config_path, "read")?;
        let config: GemmaConfig =
            serde_json::from_slice(&config_bytes).map_err(|source| Gemma4TargetError::Json {
                path: config_path.clone(),
                source,
            })?;
        validate_config(&config)?;

        let weights_path = root.join(WEIGHTS_FILE);
        let manifest = SafetensorsManifest::read(&weights_path)?;
        let layers =
            validate_tensor_manifest(&manifest.entries, manifest.data_start, manifest.file_len)?;

        Ok(Self {
            root,
            weights_path,
            safetensors_layout_digest: *blake3::hash(&manifest.header_bytes).as_bytes(),
            weights_file_len: manifest.file_len,
            layers,
        })
    }

    /// Return the validated checkpoint directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return a digest of the safetensors JSON layout.
    ///
    /// This digest identifies tensor names, shapes, and offsets. It does not replace
    /// the attempt-specific matrix commitments that bind weight values in consensus.
    pub const fn safetensors_layout_digest(&self) -> [u8; 32] {
        self.safetensors_layout_digest
    }

    /// Return the validated safetensors file length.
    pub const fn weights_file_len(&self) -> u64 {
        self.weights_file_len
    }

    /// Load one INT7 MLP weight and zero-pad it into peak `B` column-major layout.
    ///
    /// Safetensors stores the logical weight as `[out, in]` row-major. The same byte
    /// order is column-major for Pearl's conceptual `[in, out]` matrix. Each logical
    /// output column is extended from 5,376 to 8,192 elements, and the remaining
    /// output columns through 32,768 are zero.
    pub fn load_peak_b_col_major(
        &self,
        layer: usize,
        projection: Gemma4MlpProjection,
    ) -> Result<Vec<i8>, Gemma4TargetError> {
        validate_layer(layer)?;
        let tensor_name = projection.tensor_name(layer)?;
        let slice = self.layers[layer].get(projection);
        let expected_logical_bytes = GEMMA4_INTERMEDIATE_SIZE
            .checked_mul(GEMMA4_HIDDEN_SIZE)
            .ok_or_else(|| {
                Gemma4TargetError::InvalidManifest("Gemma weight size overflow".into())
            })?;
        if slice.byte_len != expected_logical_bytes as u64 {
            return Err(Gemma4TargetError::InvalidManifest(format!(
                "{tensor_name} has {} bytes; expected {expected_logical_bytes}",
                slice.byte_len
            )));
        }

        let mut file = File::open(&self.weights_path).map_err(|source| Gemma4TargetError::Io {
            operation: "open",
            path: self.weights_path.clone(),
            source,
        })?;
        file.seek(SeekFrom::Start(slice.absolute_offset))
            .map_err(|source| Gemma4TargetError::Io {
                operation: "seek",
                path: self.weights_path.clone(),
                source,
            })?;
        let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
        let physical_len = GEMMA4_PEAK_N
            .checked_mul(GEMMA4_PEAK_K)
            .ok_or_else(|| Gemma4TargetError::InvalidManifest("peak B size overflow".into()))?;
        let mut padded = vec![0i8; physical_len];
        let mut logical_row = vec![0u8; GEMMA4_HIDDEN_SIZE];

        for output in 0..GEMMA4_INTERMEDIATE_SIZE {
            reader
                .read_exact(&mut logical_row)
                .map_err(|source| Gemma4TargetError::Io {
                    operation: "read tensor",
                    path: self.weights_path.clone(),
                    source,
                })?;
            let row_start = output * GEMMA4_PEAK_K;
            for (input, byte) in logical_row.iter().copied().enumerate() {
                let value = byte as i8;
                if !(-64..=64).contains(&value) {
                    return Err(Gemma4TargetError::InvalidOperands(format!(
                        "{tensor_name}[{output},{input}] is {value}; INT7 operands must be in [-64, 64]"
                    )));
                }
                padded[row_start + input] = value;
            }
        }
        Ok(padded)
    }

    /// Zero-pad token-major INT7 activations into peak `A` row-major layout.
    pub fn pad_peak_a_row_major(
        tokens: usize,
        quantized_activations: &[i8],
    ) -> Result<Vec<i8>, Gemma4TargetError> {
        if tokens == 0 || tokens > GEMMA4_MAX_TOKENS {
            return Err(Gemma4TargetError::InvalidOperands(format!(
                "Gemma token rows must be in 1..={GEMMA4_MAX_TOKENS}; got {tokens}"
            )));
        }
        pad_i8_rows(
            quantized_activations, tokens, GEMMA4_HIDDEN_SIZE, GEMMA4_MAX_TOKENS, GEMMA4_PEAK_K,
            "Gemma activation",
        )
    }
}

/// Gemma target validation failure.
#[derive(Debug, Error)]
pub enum Gemma4TargetError {
    #[error("cannot {operation} {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid Gemma 4 checkpoint configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid Gemma 4 safetensors manifest: {0}")]
    InvalidManifest(String),
    #[error("invalid Gemma 4 peak operands: {0}")]
    InvalidOperands(String),
}

#[derive(Debug, Deserialize)]
struct GemmaConfig {
    architectures: Vec<String>,
    model_type: String,
    quantization_config: QuantizationConfig,
    text_config: TextConfig,
}

#[derive(Debug, Deserialize)]
struct QuantizationConfig {
    config_groups: BTreeMap<String, QuantizationGroup>,
    format: String,
    quant_method: String,
    quantization_status: String,
    version: String,
}

#[derive(Debug, Deserialize)]
struct QuantizationGroup {
    format: String,
    input_activations: QuantizationArgs,
    targets: Vec<String>,
    weights: QuantizationArgs,
}

#[derive(Debug, Deserialize)]
struct QuantizationArgs {
    block_structure: Option<Vec<u32>>,
    dynamic: bool,
    group_size: Option<u32>,
    num_bits: u8,
    strategy: String,
    symmetric: bool,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct TextConfig {
    enable_moe_block: bool,
    hidden_size: usize,
    intermediate_size: usize,
    layer_types: Vec<String>,
    model_type: String,
    num_hidden_layers: usize,
}

fn validate_config(config: &GemmaConfig) -> Result<(), Gemma4TargetError> {
    require_config(
        config.architectures == [GEMMA4_ARCHITECTURE],
        "architectures must contain only Gemma4ForConditionalGeneration",
    )?;
    require_config(
        config.model_type == GEMMA4_MODEL_TYPE,
        "model_type must be gemma4",
    )?;
    require_config(
        config.text_config.model_type == GEMMA4_TEXT_MODEL_TYPE,
        "text_config.model_type must be gemma4_text",
    )?;
    require_config(
        config.text_config.hidden_size == GEMMA4_HIDDEN_SIZE,
        "text_config.hidden_size must be 5376",
    )?;
    require_config(
        config.text_config.intermediate_size == GEMMA4_INTERMEDIATE_SIZE,
        "text_config.intermediate_size must be 21504",
    )?;
    require_config(
        config.text_config.num_hidden_layers == GEMMA4_LAYERS,
        "text_config.num_hidden_layers must be 60",
    )?;
    require_config(
        !config.text_config.enable_moe_block, "text_config.enable_moe_block must be false",
    )?;
    require_config(
        config.text_config.layer_types.len() == GEMMA4_LAYERS,
        "text_config.layer_types must contain 60 entries",
    )?;
    for (layer, kind) in config.text_config.layer_types.iter().enumerate() {
        let expected = if layer % 6 == 5 {
            "full_attention"
        } else {
            "sliding_attention"
        };
        require_config(
            kind == expected,
            format!("text_config.layer_types[{layer}] must be {expected}"),
        )?;
    }

    let quant = &config.quantization_config;
    require_config(
        quant.format == "mixed-precision",
        "quantization format must be mixed-precision",
    )?;
    require_config(quant.quant_method == "pearl", "quant_method must be pearl")?;
    require_config(
        quant.quantization_status == "compressed",
        "quantization_status must be compressed",
    )?;
    require_config(
        quant.version == REFERENCE_QUANTIZATION_VERSION,
        format!("quantization version must be {REFERENCE_QUANTIZATION_VERSION}"),
    )?;
    require_config(
        quant.config_groups.len() == 2,
        "quantization config must contain exactly group_0 and group_1",
    )?;

    let fp8 = quant
        .config_groups
        .get("group_0")
        .ok_or_else(|| Gemma4TargetError::InvalidConfig("group_0 is missing".into()))?;
    require_config(
        fp8.format == "float-quantized",
        "group_0 format must be float-quantized",
    )?;
    require_targets(
        &fp8.targets,
        &[
            r"re:.*self_attn\.q_proj", r"re:.*self_attn\.k_proj", r"re:.*self_attn\.v_proj",
            r"re:.*self_attn\.qkv_proj", r"re:.*mlp\.down_proj",
        ],
        "group_0",
    )?;
    require_quant_args(
        &fp8.input_activations,
        QuantArgsExpectation {
            kind: "float",
            bits: 8,
            strategy: "group",
            dynamic: true,
            symmetric: true,
            group_size: Some(128),
            block_structure: None,
        },
        "group_0 input activations",
    )?;
    require_quant_args(
        &fp8.weights,
        QuantArgsExpectation {
            kind: "float",
            bits: 8,
            strategy: "block",
            dynamic: false,
            symmetric: true,
            group_size: None,
            block_structure: Some(&[128, 128]),
        },
        "group_0 weights",
    )?;

    let int7 = quant
        .config_groups
        .get("group_1")
        .ok_or_else(|| Gemma4TargetError::InvalidConfig("group_1 is missing".into()))?;
    require_config(
        int7.format == "int-quantized",
        "group_1 format must be int-quantized",
    )?;
    require_targets(&int7.targets, &["Linear"], "group_1")?;
    require_quant_args(
        &int7.input_activations,
        QuantArgsExpectation {
            kind: "int",
            bits: 7,
            strategy: "token",
            dynamic: true,
            symmetric: true,
            group_size: None,
            block_structure: None,
        },
        "group_1 input activations",
    )?;
    require_quant_args(
        &int7.weights,
        QuantArgsExpectation {
            kind: "int",
            bits: 7,
            strategy: "channel",
            dynamic: false,
            symmetric: true,
            group_size: None,
            block_structure: None,
        },
        "group_1 weights",
    )?;
    Ok(())
}

fn require_config(condition: bool, message: impl Into<String>) -> Result<(), Gemma4TargetError> {
    if condition {
        Ok(())
    } else {
        Err(Gemma4TargetError::InvalidConfig(message.into()))
    }
}

fn require_targets(
    actual: &[String],
    expected: &[&str],
    group: &str,
) -> Result<(), Gemma4TargetError> {
    let actual: BTreeSet<&str> = actual.iter().map(String::as_str).collect();
    let expected: BTreeSet<&str> = expected.iter().copied().collect();
    require_config(
        actual == expected,
        format!("{group} targets do not match the target profile"),
    )
}

#[derive(Clone, Copy)]
struct QuantArgsExpectation<'a> {
    kind: &'a str,
    bits: u8,
    strategy: &'a str,
    dynamic: bool,
    symmetric: bool,
    group_size: Option<u32>,
    block_structure: Option<&'a [u32]>,
}

fn require_quant_args(
    actual: &QuantizationArgs,
    expected: QuantArgsExpectation<'_>,
    label: &str,
) -> Result<(), Gemma4TargetError> {
    require_config(
        actual.kind == expected.kind,
        format!("{label} type must be {}", expected.kind),
    )?;
    require_config(
        actual.num_bits == expected.bits,
        format!("{label} num_bits must be {}", expected.bits),
    )?;
    require_config(
        actual.strategy == expected.strategy,
        format!("{label} strategy must be {}", expected.strategy),
    )?;
    require_config(
        actual.dynamic == expected.dynamic,
        format!("{label} dynamic flag is invalid"),
    )?;
    require_config(
        actual.symmetric == expected.symmetric,
        format!("{label} symmetric flag is invalid"),
    )?;
    require_config(
        actual.group_size == expected.group_size,
        format!("{label} group_size is invalid"),
    )?;
    require_config(
        actual.block_structure.as_deref() == expected.block_structure,
        format!("{label} block_structure is invalid"),
    )
}

struct SafetensorsManifest {
    entries: BTreeMap<String, serde_json::Value>,
    header_bytes: Vec<u8>,
    data_start: u64,
    file_len: u64,
}

impl SafetensorsManifest {
    fn read(path: &Path) -> Result<Self, Gemma4TargetError> {
        let mut file = File::open(path).map_err(|source| Gemma4TargetError::Io {
            operation: "open",
            path: path.to_path_buf(),
            source,
        })?;
        let file_len = file
            .metadata()
            .map_err(|source| Gemma4TargetError::Io {
                operation: "stat",
                path: path.to_path_buf(),
                source,
            })?
            .len();
        let mut length_bytes = [0u8; 8];
        file.read_exact(&mut length_bytes)
            .map_err(|source| Gemma4TargetError::Io {
                operation: "read header length from",
                path: path.to_path_buf(),
                source,
            })?;
        let header_len = u64::from_le_bytes(length_bytes);
        if header_len == 0 || header_len > MAX_SAFETENSORS_HEADER_BYTES {
            return Err(Gemma4TargetError::InvalidManifest(format!(
                "header length {header_len} is outside 1..={MAX_SAFETENSORS_HEADER_BYTES}"
            )));
        }
        let data_start = 8u64
            .checked_add(header_len)
            .ok_or_else(|| Gemma4TargetError::InvalidManifest("header offset overflow".into()))?;
        if data_start > file_len {
            return Err(Gemma4TargetError::InvalidManifest(format!(
                "header ends at {data_start}, past file length {file_len}"
            )));
        }
        let mut header_bytes = vec![0u8; header_len as usize];
        file.read_exact(&mut header_bytes)
            .map_err(|source| Gemma4TargetError::Io {
                operation: "read header from",
                path: path.to_path_buf(),
                source,
            })?;
        let entries: BTreeMap<String, serde_json::Value> = serde_json::from_slice(&header_bytes)
            .map_err(|source| Gemma4TargetError::Json {
                path: path.to_path_buf(),
                source,
            })?;
        Ok(Self {
            entries,
            header_bytes,
            data_start,
            file_len,
        })
    }
}

#[derive(Debug, Deserialize)]
struct SafetensorEntry {
    dtype: String,
    shape: Vec<u64>,
    data_offsets: [u64; 2],
}

fn validate_tensor_manifest(
    entries: &BTreeMap<String, serde_json::Value>,
    data_start: u64,
    file_len: u64,
) -> Result<Vec<LayerTensorSlices>, Gemma4TargetError> {
    let mut expected_int7 = BTreeSet::new();
    let mut layers = Vec::with_capacity(GEMMA4_LAYERS);
    for layer in 0..GEMMA4_LAYERS {
        let gate_name = Gemma4MlpProjection::Gate.tensor_name(layer)?;
        let up_name = Gemma4MlpProjection::Up.tensor_name(layer)?;
        let gate = tensor_slice(
            entries,
            &gate_name,
            &[GEMMA4_INTERMEDIATE_SIZE as u64, GEMMA4_HIDDEN_SIZE as u64],
            data_start,
            file_len,
        )?;
        let up = tensor_slice(
            entries,
            &up_name,
            &[GEMMA4_INTERMEDIATE_SIZE as u64, GEMMA4_HIDDEN_SIZE as u64],
            data_start,
            file_len,
        )?;
        expected_int7.insert(gate_name);
        expected_int7.insert(up_name);

        let attention_input = if layer % 6 == 5 {
            FULL_ATTENTION_INPUT_SIZE
        } else {
            SLIDING_ATTENTION_INPUT_SIZE
        };
        let o_name = format!("model.language_model.layers.{layer}.self_attn.o_proj.weight");
        tensor_slice(
            entries,
            &o_name,
            &[GEMMA4_HIDDEN_SIZE as u64, attention_input],
            data_start,
            file_len,
        )?;
        expected_int7.insert(o_name);
        layers.push(LayerTensorSlices { gate, up });
    }

    let actual_int7: BTreeSet<String> = entries
        .iter()
        .filter(|(_, value)| value.get("dtype").and_then(serde_json::Value::as_str) == Some("I8"))
        .map(|(name, _)| name.clone())
        .collect();
    if actual_int7 != expected_int7 {
        return Err(Gemma4TargetError::InvalidManifest(format!(
            "INT7 tensor set has {} entries; the target profile requires {} exact gate, up, and o-projection tensors",
            actual_int7.len(),
            expected_int7.len()
        )));
    }
    Ok(layers)
}

fn tensor_slice(
    entries: &BTreeMap<String, serde_json::Value>,
    name: &str,
    expected_shape: &[u64],
    data_start: u64,
    file_len: u64,
) -> Result<TensorSlice, Gemma4TargetError> {
    let value = entries
        .get(name)
        .ok_or_else(|| Gemma4TargetError::InvalidManifest(format!("missing tensor {name}")))?;
    let entry: SafetensorEntry = serde_json::from_value(value.clone()).map_err(|source| {
        Gemma4TargetError::InvalidManifest(format!("invalid tensor {name}: {source}"))
    })?;
    if entry.dtype != "I8" {
        return Err(Gemma4TargetError::InvalidManifest(format!(
            "{name} dtype is {}; expected I8",
            entry.dtype
        )));
    }
    if entry.shape != expected_shape {
        return Err(Gemma4TargetError::InvalidManifest(format!(
            "{name} shape is {:?}; expected {expected_shape:?}",
            entry.shape
        )));
    }
    let expected_bytes = expected_shape
        .iter()
        .try_fold(1u64, |size, dimension| size.checked_mul(*dimension));
    let expected_bytes = expected_bytes
        .ok_or_else(|| Gemma4TargetError::InvalidManifest(format!("{name} size overflow")))?;
    let [start, end] = entry.data_offsets;
    let byte_len = end.checked_sub(start).ok_or_else(|| {
        Gemma4TargetError::InvalidManifest(format!("{name} offsets are reversed"))
    })?;
    if byte_len != expected_bytes {
        return Err(Gemma4TargetError::InvalidManifest(format!(
            "{name} data length is {byte_len}; expected {expected_bytes}"
        )));
    }
    let absolute_offset = data_start
        .checked_add(start)
        .ok_or_else(|| Gemma4TargetError::InvalidManifest(format!("{name} offset overflow")))?;
    let absolute_end = data_start
        .checked_add(end)
        .ok_or_else(|| Gemma4TargetError::InvalidManifest(format!("{name} end offset overflow")))?;
    if absolute_end > file_len {
        return Err(Gemma4TargetError::InvalidManifest(format!(
            "{name} ends at {absolute_end}, past file length {file_len}"
        )));
    }
    Ok(TensorSlice {
        absolute_offset,
        byte_len,
    })
}

fn validate_layer(layer: usize) -> Result<(), Gemma4TargetError> {
    if layer < GEMMA4_LAYERS {
        Ok(())
    } else {
        Err(Gemma4TargetError::InvalidOperands(format!(
            "Gemma decoder layer must be in 0..{GEMMA4_LAYERS}; got {layer}"
        )))
    }
}

fn pad_i8_rows(
    source: &[i8],
    logical_rows: usize,
    logical_width: usize,
    physical_rows: usize,
    physical_width: usize,
    label: &str,
) -> Result<Vec<i8>, Gemma4TargetError> {
    if logical_rows > physical_rows || logical_width > physical_width {
        return Err(Gemma4TargetError::InvalidOperands(format!(
            "{label} logical shape {logical_rows}x{logical_width} exceeds physical shape {physical_rows}x{physical_width}"
        )));
    }
    let expected = logical_rows
        .checked_mul(logical_width)
        .ok_or_else(|| Gemma4TargetError::InvalidOperands(format!("{label} length overflow")))?;
    if source.len() != expected {
        return Err(Gemma4TargetError::InvalidOperands(format!(
            "{label} has {} values; expected {expected}",
            source.len()
        )));
    }
    let physical_len = physical_rows.checked_mul(physical_width).ok_or_else(|| {
        Gemma4TargetError::InvalidOperands(format!("{label} padded length overflow"))
    })?;
    let mut padded = vec![0i8; physical_len];
    for row in 0..logical_rows {
        let source_row = &source[row * logical_width..(row + 1) * logical_width];
        if let Some((column, value)) = source_row
            .iter()
            .copied()
            .enumerate()
            .find(|(_, value)| !(-64..=64).contains(value))
        {
            return Err(Gemma4TargetError::InvalidOperands(format!(
                "{label}[{row},{column}] is {value}; INT7 operands must be in [-64, 64]"
            )));
        }
        let destination = &mut padded[row * physical_width..row * physical_width + logical_width];
        destination.copy_from_slice(source_row);
    }
    Ok(padded)
}

fn read_file(path: &Path, operation: &'static str) -> Result<Vec<u8>, Gemma4TargetError> {
    std::fs::read(path).map_err(|source| Gemma4TargetError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_mapping_keeps_the_consensus_profile() {
        assert_eq!(GEMMA4_MAX_TOKENS, 4_096);
        assert_eq!(GEMMA4_PEAK_K, 8_192);
        assert_eq!(GEMMA4_PEAK_N, 32_768);
        assert!(GEMMA4_HIDDEN_SIZE <= GEMMA4_PEAK_K);
        assert!(GEMMA4_INTERMEDIATE_SIZE <= GEMMA4_PEAK_N);
        PEAK_PRODUCTION_PARAMS
            .validate_prod_envelope()
            .expect("the shared peak profile must stay consensus-admitted");
    }

    #[test]
    fn projection_names_match_the_checkpoint_layout() {
        assert_eq!(
            Gemma4MlpProjection::Gate.tensor_name(0).unwrap(),
            "model.language_model.layers.0.mlp.gate_proj.weight"
        );
        assert_eq!(
            Gemma4MlpProjection::Up.tensor_name(59).unwrap(),
            "model.language_model.layers.59.mlp.up_proj.weight"
        );
        assert!(Gemma4MlpProjection::Gate.tensor_name(60).is_err());
    }

    #[test]
    fn row_padding_preserves_logical_values_and_zeroes_the_rest() {
        let padded = pad_i8_rows(&[-64, 0, 64, 1, 2, 3], 2, 3, 4, 5, "test").unwrap();
        assert_eq!(
            padded,
            vec![
                -64, 0, 64, 0, 0, // logical row 0
                1, 2, 3, 0, 0, // logical row 1
                0, 0, 0, 0, 0, // padded row 2
                0, 0, 0, 0, 0, // padded row 3
            ]
        );
    }

    #[test]
    fn activation_mapping_uses_the_complete_peak_shape() {
        let mut logical = vec![0i8; GEMMA4_HIDDEN_SIZE];
        logical[0] = -64;
        logical[GEMMA4_HIDDEN_SIZE - 1] = 64;
        let padded = Gemma4Checkpoint::pad_peak_a_row_major(1, &logical).unwrap();
        assert_eq!(padded.len(), GEMMA4_MAX_TOKENS * GEMMA4_PEAK_K);
        assert_eq!(padded[0], -64);
        assert_eq!(padded[GEMMA4_HIDDEN_SIZE - 1], 64);
        assert!(padded[GEMMA4_HIDDEN_SIZE..].iter().all(|value| *value == 0));
        assert!(Gemma4Checkpoint::pad_peak_a_row_major(0, &[]).is_err());
        assert!(Gemma4Checkpoint::pad_peak_a_row_major(GEMMA4_MAX_TOKENS + 1, &[]).is_err());
    }

    #[test]
    fn row_padding_rejects_non_int7_values() {
        let error = pad_i8_rows(&[65], 1, 1, 1, 1, "test").unwrap_err();
        assert!(error.to_string().contains("INT7 operands"));
    }

    fn reference_checkpoint_path() -> PathBuf {
        std::env::var_os("GEMMA4_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../Gemma-4-31B-it-pearl")
            })
    }

    #[test]
    fn reference_checkpoint_manifest_matches_the_target_profile() {
        let path = reference_checkpoint_path();
        if !path.join(CONFIG_FILE).exists() || !path.join(WEIGHTS_FILE).exists() {
            eprintln!("SKIP: set GEMMA4_MODEL_DIR to the Pearl Gemma 4 checkpoint");
            return;
        }
        let checkpoint = Gemma4Checkpoint::open(&path).unwrap();
        assert_eq!(checkpoint.root(), path);
        assert_ne!(checkpoint.safetensors_layout_digest(), [0; 32]);
        assert!(checkpoint.weights_file_len() > 30_000_000_000);
    }

    #[test]
    #[ignore = "reads and expands one 115 MB checkpoint tensor"]
    fn reference_gate_weight_maps_to_peak_b() {
        let path = reference_checkpoint_path();
        let checkpoint = Gemma4Checkpoint::open(path).unwrap();
        let b = checkpoint
            .load_peak_b_col_major(0, Gemma4MlpProjection::Gate)
            .unwrap();
        assert_eq!(b.len(), GEMMA4_PEAK_N * GEMMA4_PEAK_K);
        assert!(b[..GEMMA4_HIDDEN_SIZE]
            .iter()
            .all(|value| (-64..=64).contains(value)));
        assert!(b[GEMMA4_HIDDEN_SIZE..GEMMA4_PEAK_K]
            .iter()
            .all(|value| *value == 0));
        assert!(b[GEMMA4_INTERMEDIATE_SIZE * GEMMA4_PEAK_K..]
            .iter()
            .all(|value| *value == 0));
    }
}
