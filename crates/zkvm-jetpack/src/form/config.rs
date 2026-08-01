use std::collections::BTreeMap;

use crate::form::proof::ProofVersion;
use crate::form::term::Term;

const TERM_COMPUTE: &str = "compute";
const TERM_MEMORY: &str = "memory";

#[derive(Clone, Debug)]
pub struct TableMeta {
    pub name: Term,
    pub base_width: u32,
    pub ext_width: u32,
    pub mega_ext_width: u32,
    pub full_width: u32,
    pub num_randomizers: u32,
}

#[derive(Debug)]
pub enum ConfigError {
    UnknownTable(Term),
}

impl TableMeta {
    pub const fn new(
        name: Term,
        base_width: u32,
        ext_width: u32,
        mega_ext_width: u32,
        num_randomizers: u32,
    ) -> Self {
        Self {
            name,
            base_width,
            ext_width,
            mega_ext_width,
            full_width: base_width + ext_width + mega_ext_width,
            num_randomizers,
        }
    }
}

static V0_V1_TABLES: [TableMeta; 2] = [
    TableMeta::new(Term::from_static(TERM_COMPUTE), 11, 165, 18, 1),
    TableMeta::new(Term::from_static(TERM_MEMORY), 14, 33, 24, 1),
];

static V2_TABLES: [TableMeta; 2] = [
    TableMeta::new(Term::from_static(TERM_COMPUTE), 11, 165, 18, 1),
    TableMeta::new(Term::from_static(TERM_MEMORY), 14, 30, 24, 1),
];

static CORE_NAMES_V0_V1: [Term; 2] =
    [Term::from_static(TERM_COMPUTE), Term::from_static(TERM_MEMORY)];
static CORE_NAMES_V2: [Term; 2] = [Term::from_static(TERM_COMPUTE), Term::from_static(TERM_MEMORY)];

fn tables_for(version: &ProofVersion) -> &'static [TableMeta] {
    match version {
        ProofVersion::V0 | ProofVersion::V1 => &V0_V1_TABLES,
        ProofVersion::V2 | ProofVersion::V3 => &V2_TABLES,
    }
}

pub fn core_table_names(version: &ProofVersion) -> &'static [Term] {
    match version {
        ProofVersion::V0 | ProofVersion::V1 => &CORE_NAMES_V0_V1,
        ProofVersion::V2 | ProofVersion::V3 => &CORE_NAMES_V2,
    }
}

pub fn table_meta_map(version: &ProofVersion) -> BTreeMap<Term, TableMeta> {
    tables_for(version)
        .iter()
        .map(|meta| (meta.name.clone(), meta.clone()))
        .collect()
}

pub fn table_meta(version: &ProofVersion, name: &Term) -> Option<TableMeta> {
    tables_for(version)
        .iter()
        .find(|meta| &meta.name == name)
        .cloned()
}

pub fn compute_base_widths(
    version: &ProofVersion,
    override_names: Option<&[Term]>,
) -> Result<Vec<u64>, ConfigError> {
    let metas = table_meta_map(version);
    let names: Vec<Term> = match override_names {
        Some(list) => list.to_vec(),
        None => core_table_names(version).to_vec(),
    };
    names
        .into_iter()
        .map(|name| {
            metas
                .get(&name)
                .map(|meta| meta.base_width as u64)
                .ok_or(ConfigError::UnknownTable(name))
        })
        .collect()
}

pub fn compute_full_widths(
    version: &ProofVersion,
    override_names: Option<&[Term]>,
) -> Result<Vec<u64>, ConfigError> {
    let metas = table_meta_map(version);
    let names: Vec<Term> = match override_names {
        Some(list) => list.to_vec(),
        None => core_table_names(version).to_vec(),
    };
    names
        .into_iter()
        .map(|name| {
            metas
                .get(&name)
                .map(|meta| meta.full_width as u64)
                .ok_or(ConfigError::UnknownTable(name))
        })
        .collect()
}

pub fn compute_ext_widths(
    version: &ProofVersion,
    override_names: Option<&[Term]>,
) -> Result<Vec<u64>, ConfigError> {
    let metas = table_meta_map(version);
    let names: Vec<Term> = match override_names {
        Some(list) => list.to_vec(),
        None => core_table_names(version).to_vec(),
    };
    names
        .into_iter()
        .map(|name| {
            metas
                .get(&name)
                .map(|meta| meta.ext_width as u64)
                .ok_or(ConfigError::UnknownTable(name))
        })
        .collect()
}

pub fn compute_mega_ext_widths(
    version: &ProofVersion,
    override_names: Option<&[Term]>,
) -> Result<Vec<u64>, ConfigError> {
    let metas = table_meta_map(version);
    let names: Vec<Term> = match override_names {
        Some(list) => list.to_vec(),
        None => core_table_names(version).to_vec(),
    };
    names
        .into_iter()
        .map(|name| {
            metas
                .get(&name)
                .map(|meta| meta.mega_ext_width as u64)
                .ok_or(ConfigError::UnknownTable(name))
        })
        .collect()
}
