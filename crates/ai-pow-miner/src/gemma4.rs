//! Gemma 4 31B checkpoint validation and native fused gate/up operand mapping.
//!
//! Nockchain consensus admits arbitrary INT7 matrices inside the Pearl production
//! envelope. Model identity is therefore miner policy, not a consensus input. This
//! module validates the selected Pearl checkpoint and constructs the exact fused
//! Gemma MLP matrix used by the native CUDA target.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use ai_pow::params::MatmulParams;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Exact language model architecture supported by this target profile.
pub const GEMMA4_ARCHITECTURE: &str = "Gemma4ForConditionalGeneration";
/// Exact top-level model type supported by this target profile.
pub const GEMMA4_MODEL_TYPE: &str = "gemma4";
/// Exact text model type supported by this target profile.
pub const GEMMA4_TEXT_MODEL_TYPE: &str = "gemma4_text";
/// Immutable Hugging Face revision for the selected checkpoint.
pub const GEMMA4_CHECKPOINT_REVISION: &str = "f1dfba688ce6343b0433de57ca4dc0f3d1c5baa5";
/// SHA-256 of `model.safetensors` at [`GEMMA4_CHECKPOINT_REVISION`].
pub const GEMMA4_CHECKPOINT_CONTENT_DIGEST: [u8; 32] = [
    197, 156, 184, 53, 80, 245, 43, 38, 137, 60, 24, 55, 19, 53, 85, 191, 50, 25, 4, 149, 55, 44,
    224, 9, 53, 217, 137, 89, 37, 21, 255, 64,
];
/// Number of decoder layers in the selected checkpoint.
pub const GEMMA4_LAYERS: usize = 60;
/// Input width of each mineable Gemma 4 MLP projection.
pub const GEMMA4_HIDDEN_SIZE: usize = 5_376;
/// Output width of one Gemma 4 gate or up projection.
pub const GEMMA4_PROJECTION_SIZE: usize = 21_504;
/// Output width of the fused gate + up projection.
pub const GEMMA4_FUSED_OUTPUT_SIZE: usize = 2 * GEMMA4_PROJECTION_SIZE;
/// Miner-owned fused gate/up profile admitted by the unchanged consensus envelope.
pub const GEMMA4_NATIVE_PARAMS: MatmulParams = MatmulParams {
    m: 4_096,
    k: 5_376,
    n: 43_008,
    noise_rank: 128,
    tile: 16,
    spot_checks: 1,
    difficulty_bits: 0,
};
/// Maximum token rows carried by one native fused work matrix.
pub const GEMMA4_MAX_TOKENS: usize = GEMMA4_NATIVE_PARAMS.m as usize;
/// Number of rank-128 transcript cadences in one native Gemma dot product.
pub const GEMMA4_TRANSCRIPT_CADENCES: usize =
    GEMMA4_NATIVE_PARAMS.k as usize / GEMMA4_NATIVE_PARAMS.noise_rank as usize;

const CONFIG_FILE: &str = "config.json";
const WEIGHTS_FILE: &str = "model.safetensors";
const MAX_SAFETENSORS_HEADER_BYTES: u64 = 16 * 1024 * 1024;
const REFERENCE_QUANTIZATION_VERSION: &str = "0.15.0.1";
const FULL_ATTENTION_INPUT_SIZE: u64 = 16_384;
const SLIDING_ATTENTION_INPUT_SIZE: u64 = 8_192;

/// CUDA implementation selected for one exact device capability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gemma4CudaBackend {
    HopperSm90a,
    BlackwellSm120a,
}

impl Gemma4CudaBackend {
    pub const fn for_compute_capability(major: u32, minor: u32) -> Option<Self> {
        match (major, minor) {
            (9, 0) => Some(Self::HopperSm90a),
            (12, 0) => Some(Self::BlackwellSm120a),
            _ => None,
        }
    }
}

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
/// Opening a checkpoint validates `config.json` and the bounded safetensors JSON
/// header. [`Self::content_digest`] scans both files before CUDA initialization.
#[derive(Debug)]
pub struct Gemma4Checkpoint {
    root: PathBuf,
    weights_path: PathBuf,
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
            weights_file_len: manifest.file_len,
            layers,
        })
    }

    /// Return the validated checkpoint directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Hash every safetensors weight byte with SHA-256.
    pub fn content_digest(&self) -> Result<[u8; 32], Gemma4TargetError> {
        let (weights_len, weights_digest) =
            checkpoint_file_digest(&self.weights_path, "hash weights")?;
        if weights_len != self.weights_file_len {
            return Err(Gemma4TargetError::InvalidManifest(format!(
                "weights length changed from {} to {weights_len} bytes",
                self.weights_file_len
            )));
        }
        Ok(weights_digest)
    }

    /// Return the validated safetensors file length.
    pub const fn weights_file_len(&self) -> u64 {
        self.weights_file_len
    }

    /// Load one decoder layer's INT7 gate and up weights as one native `B`.
    ///
    /// Safetensors stores each logical weight as `[out, in]` row-major. The same
    /// byte order is column-major for Pearl's conceptual `[in, out]` matrix.
    /// Gate columns precede up columns, so the resulting output splits at
    /// [`GEMMA4_PROJECTION_SIZE`] without padding or transposition.
    pub fn load_fused_gate_up_b_col_major(
        &self,
        layer: usize,
    ) -> Result<Vec<i8>, Gemma4TargetError> {
        validate_layer(layer)?;
        let physical_len = GEMMA4_FUSED_OUTPUT_SIZE
            .checked_mul(GEMMA4_HIDDEN_SIZE)
            .ok_or_else(|| {
                Gemma4TargetError::InvalidManifest("fused Gemma B size overflow".into())
            })?;
        let mut fused = vec![0i8; physical_len];
        let file = File::open(&self.weights_path).map_err(|source| Gemma4TargetError::Io {
            operation: "open",
            path: self.weights_path.clone(),
            source,
        })?;
        let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
        for (projection_index, projection) in [Gemma4MlpProjection::Gate, Gemma4MlpProjection::Up]
            .into_iter()
            .enumerate()
        {
            let tensor_name = projection.tensor_name(layer)?;
            let slice = self.layers[layer].get(projection);
            let output_base = projection_index * GEMMA4_PROJECTION_SIZE;
            read_projection_into(
                &mut reader, &self.weights_path, slice, &tensor_name, output_base, &mut fused,
            )?;
        }
        Ok(fused)
    }

    /// Zero-pad token-major INT7 activations to the native 4,096-row batch.
    ///
    /// The common dimension remains the model's exact 5,376 values. Only absent
    /// token rows are zero, so a full batch performs no padded MACs.
    pub fn build_native_a_row_major(
        tokens: usize,
        quantized_activations: &[i8],
    ) -> Result<Vec<i8>, Gemma4TargetError> {
        if tokens == 0 || tokens > GEMMA4_MAX_TOKENS {
            return Err(Gemma4TargetError::InvalidOperands(format!(
                "Gemma token rows must be in 1..={GEMMA4_MAX_TOKENS}; got {tokens}"
            )));
        }
        pad_i8_rows(
            quantized_activations, tokens, GEMMA4_HIDDEN_SIZE, GEMMA4_MAX_TOKENS,
            GEMMA4_HIDDEN_SIZE, "Gemma activation",
        )
    }
}

fn checkpoint_file_digest(
    path: &Path,
    operation: &'static str,
) -> Result<(u64, [u8; 32]), Gemma4TargetError> {
    let file = File::open(path).map_err(|source| Gemma4TargetError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })?;
    let expected_len = file
        .metadata()
        .map_err(|source| Gemma4TargetError::Io {
            operation,
            path: path.to_path_buf(),
            source,
        })?
        .len();
    let mut reader = BufReader::with_capacity(8 * 1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 8 * 1024 * 1024];
    let mut read_len = 0u64;
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|source| Gemma4TargetError::Io {
                operation,
                path: path.to_path_buf(),
                source,
            })?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        read_len = read_len
            .checked_add(count as u64)
            .ok_or_else(|| Gemma4TargetError::InvalidManifest("file length overflow".into()))?;
    }
    if read_len != expected_len {
        return Err(Gemma4TargetError::InvalidManifest(format!(
            "{} changed length while hashing: expected {expected_len}, read {read_len}",
            path.display()
        )));
    }
    Ok((read_len, hasher.finalize().into()))
}

fn read_projection_into(
    reader: &mut BufReader<File>,
    weights_path: &Path,
    slice: TensorSlice,
    tensor_name: &str,
    output_base: usize,
    fused: &mut [i8],
) -> Result<(), Gemma4TargetError> {
    let expected_bytes = GEMMA4_PROJECTION_SIZE
        .checked_mul(GEMMA4_HIDDEN_SIZE)
        .ok_or_else(|| Gemma4TargetError::InvalidManifest("Gemma weight size overflow".into()))?;
    if slice.byte_len != expected_bytes as u64 {
        return Err(Gemma4TargetError::InvalidManifest(format!(
            "{tensor_name} has {} bytes; expected {expected_bytes}",
            slice.byte_len
        )));
    }
    reader
        .seek(SeekFrom::Start(slice.absolute_offset))
        .map_err(|source| Gemma4TargetError::Io {
            operation: "seek",
            path: weights_path.to_path_buf(),
            source,
        })?;
    let mut logical_row = vec![0u8; GEMMA4_HIDDEN_SIZE];
    for output in 0..GEMMA4_PROJECTION_SIZE {
        reader
            .read_exact(&mut logical_row)
            .map_err(|source| Gemma4TargetError::Io {
                operation: "read tensor",
                path: weights_path.to_path_buf(),
                source,
            })?;
        let fused_row = output_base + output;
        let row_start = fused_row * GEMMA4_HIDDEN_SIZE;
        for (input, byte) in logical_row.iter().copied().enumerate() {
            let value = byte as i8;
            if !(-64..=64).contains(&value) {
                return Err(Gemma4TargetError::InvalidOperands(format!(
                    "{tensor_name}[{output},{input}] is {value}; INT7 operands must be in [-64, 64]"
                )));
            }
            fused[row_start + input] = value;
        }
    }
    Ok(())
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
        config.text_config.intermediate_size == GEMMA4_PROJECTION_SIZE,
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
            &[GEMMA4_PROJECTION_SIZE as u64, GEMMA4_HIDDEN_SIZE as u64],
            data_start,
            file_len,
        )?;
        let up = tensor_slice(
            entries,
            &up_name,
            &[GEMMA4_PROJECTION_SIZE as u64, GEMMA4_HIDDEN_SIZE as u64],
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
    fn native_mapping_uses_the_fused_consensus_profile() {
        assert_eq!(GEMMA4_MAX_TOKENS, 4_096);
        assert_eq!(GEMMA4_NATIVE_PARAMS.k, 5_376);
        assert_eq!(GEMMA4_NATIVE_PARAMS.n, 43_008);
        assert_eq!(GEMMA4_NATIVE_PARAMS.noise_rank, 128);
        assert_eq!(GEMMA4_NATIVE_PARAMS.tile, 16);
        assert_eq!(GEMMA4_NATIVE_PARAMS.spot_checks, 1);
        assert_eq!(GEMMA4_NATIVE_PARAMS.difficulty_bits, 0);
        assert_eq!(GEMMA4_NATIVE_PARAMS.row_tiles(), 256);
        assert_eq!(GEMMA4_NATIVE_PARAMS.col_tiles(), 2_688);
        assert_eq!(GEMMA4_NATIVE_PARAMS.num_tiles(), 688_128);
        assert_eq!(GEMMA4_NATIVE_PARAMS.num_stripes(), 42);
        assert_eq!(GEMMA4_FUSED_OUTPUT_SIZE, 2 * GEMMA4_PROJECTION_SIZE);
        GEMMA4_NATIVE_PARAMS
            .validate_prod_envelope()
            .expect("native Gemma profile must stay consensus-admitted");
    }

    #[test]
    fn backend_dispatch_accepts_only_exact_supported_capabilities() {
        assert_eq!(
            Gemma4CudaBackend::for_compute_capability(9, 0),
            Some(Gemma4CudaBackend::HopperSm90a)
        );
        assert_eq!(
            Gemma4CudaBackend::for_compute_capability(12, 0),
            Some(Gemma4CudaBackend::BlackwellSm120a)
        );
        for capability in [(8, 9), (9, 1), (10, 0), (12, 1)] {
            assert_eq!(
                Gemma4CudaBackend::for_compute_capability(capability.0, capability.1),
                None
            );
        }
    }

    #[test]
    fn forty_two_cadence_transcript_matches_known_answer() {
        let x_steps: Vec<i32> = (0..GEMMA4_TRANSCRIPT_CADENCES)
            .map(|step| ((step as u32).wrapping_mul(0x9e37_79b9) ^ 0xa5a5_5a5a) as i32)
            .collect();
        let state = ai_pow::matmul::TileState::from_x_steps(&x_steps);
        assert_eq!(
            state.0.map(|word| word as u32),
            [
                0x51e5_b0c9, 0x058f_c68d, 0x7e37_4924, 0xab90_d1bb, 0x7a18_7992, 0x51a5_e6e9,
                0xcc2d_6a80, 0x3eb6_e727, 0x923e_6afe, 0x68c7_0655, 0x476b_17e1, 0xe669_a1e1,
                0x766a_d381, 0xc76a_6601, 0x916d_99a1, 0x2263_5b21,
            ]
        );
    }
    #[test]
    fn pearl_denoising_and_bf16_output_kat() {
        const A: [[i32; 4]; 2] = [[1, -2, 3, 4], [-3, 2, 1, -1]];
        const B: [[i32; 4]; 3] = [[2, 1, -1, 3], [-2, 4, 1, 0], [3, -1, 2, -2]];
        const E_AL: [[i32; 2]; 2] = [[2, -1], [-2, 3]];
        const E_AR: [[i32; 2]; 4] = [[1, 0], [0, 1], [-1, 0], [0, -1]];
        const E_BR: [[i32; 2]; 3] = [[1, 2], [-1, 1], [2, -2]];
        const E_BL: [[i32; 2]; 4] = [[0, 1], [1, 0], [0, -1], [-1, 0]];
        const A_SCALES: [f32; 2] = [0.5, 1.25];
        const B_SCALES: [f32; 3] = [0.25, 2.0, -0.75];

        let mut a_prime = A;
        for row in 0..2 {
            for k in 0..4 {
                for rank in 0..2 {
                    a_prime[row][k] += E_AL[row][rank] * E_AR[k][rank];
                }
            }
        }
        let mut b_prime = B;
        for row in 0..3 {
            for k in 0..4 {
                for rank in 0..2 {
                    b_prime[row][k] += E_BR[row][rank] * E_BL[k][rank];
                }
            }
        }
        assert_eq!(a_prime, [[3, -3, 1, 5], [-5, 5, 3, -4]]);
        assert_eq!(b_prime, [[4, 2, -3, 2], [-1, 3, 0, 1], [1, 1, 4, -4]]);

        let mut noised = [[0i32; 3]; 2];
        let mut ax_ebl = [[0i32; 2]; 2];
        let mut ear_b_prime = [[0i32; 2]; 3];
        for row in 0..2 {
            for col in 0..3 {
                for k in 0..4 {
                    noised[row][col] += a_prime[row][k] * b_prime[col][k];
                }
            }
            for rank in 0..2 {
                for k in 0..4 {
                    ax_ebl[row][rank] += A[row][k] * E_BL[k][rank];
                }
            }
        }
        for col in 0..3 {
            for rank in 0..2 {
                for k in 0..4 {
                    ear_b_prime[col][rank] += b_prime[col][k] * E_AR[k][rank];
                }
            }
        }
        assert_eq!(noised, [[13, -7, -16], [-27, 16, 28]]);
        assert_eq!(ax_ebl, [[-6, -2], [3, -4]]);
        assert_eq!(ear_b_prime, [[7, 0], [-1, 2], [-3, 5]]);

        let mut clean = noised;
        let mut output = [[0u16; 3]; 2];
        for row in 0..2 {
            for col in 0..3 {
                for rank in 0..2 {
                    clean[row][col] -= E_AL[row][rank] * ear_b_prime[col][rank]
                        + ax_ebl[row][rank] * E_BR[col][rank];
                }
                output[row][col] =
                    f32_to_bf16_bits(clean[row][col] as f32 * A_SCALES[row] * B_SCALES[col]);
            }
        }
        assert_eq!(clean, [[9, -7, 3], [-8, 15, -7]]);
        assert_eq!(output, [[0x3f90, 0xc0e0, 0xbf90], [0xc020, 0x4216, 0x40d2]]);
    }

    fn f32_to_bf16_bits(value: f32) -> u16 {
        let bits = value.to_bits();
        let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1));
        (rounded >> 16) as u16
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
    fn activation_mapping_uses_the_native_common_dimension() {
        let mut logical = vec![0i8; GEMMA4_HIDDEN_SIZE];
        logical[0] = -64;
        logical[GEMMA4_HIDDEN_SIZE - 1] = 64;
        let padded = Gemma4Checkpoint::build_native_a_row_major(1, &logical).unwrap();
        assert_eq!(padded.len(), GEMMA4_MAX_TOKENS * GEMMA4_HIDDEN_SIZE);
        assert_eq!(padded[0], -64);
        assert_eq!(padded[GEMMA4_HIDDEN_SIZE - 1], 64);
        assert!(padded[GEMMA4_HIDDEN_SIZE..].iter().all(|value| *value == 0));
        assert!(Gemma4Checkpoint::build_native_a_row_major(0, &[]).is_err());
        assert!(Gemma4Checkpoint::build_native_a_row_major(GEMMA4_MAX_TOKENS + 1, &[]).is_err());
    }

    #[test]
    fn row_padding_rejects_non_int7_values() {
        let error = pad_i8_rows(&[65], 1, 1, 1, 1, "test").unwrap_err();
        assert!(error.to_string().contains("INT7 operands"));
    }

    #[test]
    fn checkpoint_content_digest_binds_every_weight_byte() {
        let mut weights: Vec<u8> = (0..64).collect();
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nockchain-gemma4-content-{}-{unique}.safetensors",
            std::process::id()
        ));
        std::fs::write(&path, &weights).unwrap();
        let (length, digest) = checkpoint_file_digest(&path, "hash test weights").unwrap();
        assert_eq!(length, 64);
        assert_eq!(
            hex::encode(digest),
            "fdeab9acf3710362bd2658cdc9a29e8f9c757fcf9811603a8c447cd1d9151108"
        );

        weights[31] ^= 1;
        std::fs::write(&path, &weights).unwrap();
        let (_, changed) = checkpoint_file_digest(&path, "hash changed test weights").unwrap();
        std::fs::remove_file(path).unwrap();
        assert_ne!(changed, digest);
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
        assert!(checkpoint.weights_file_len() > 30_000_000_000);
    }

    #[test]
    #[ignore = "reads and concatenates two 115 MB checkpoint tensors"]
    fn reference_gate_up_weights_map_to_native_b() {
        let path = reference_checkpoint_path();
        let checkpoint = Gemma4Checkpoint::open(path).unwrap();
        let b = checkpoint.load_fused_gate_up_b_col_major(0).unwrap();
        assert_eq!(b.len(), GEMMA4_FUSED_OUTPUT_SIZE * GEMMA4_HIDDEN_SIZE);
        assert!(b.iter().all(|value| (-64..=64).contains(value)));
        let up_start = GEMMA4_PROJECTION_SIZE * GEMMA4_HIDDEN_SIZE;
        assert_ne!(
            &b[..GEMMA4_HIDDEN_SIZE],
            &b[up_start..up_start + GEMMA4_HIDDEN_SIZE],
            "gate and up columns must come from distinct tensors"
        );
    }
}
