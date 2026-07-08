//! Mining-reward configuration types, parsed from CLI flags on the
//! miner binary and pushed to the node via `MiningWire::SetPubKey`.
//!
//! Lifted verbatim from `crates/nockchain/src/mining.rs`. The
//! `FromStr` impls are used by `clap`'s `value_parser!`.

use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct MiningKeyConfig {
    pub share: u64,
    pub m: u64,
    pub keys: Vec<String>,
}

impl FromStr for MiningKeyConfig {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Expected format: "share,m:key1,key2,key3"
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err("Invalid format. Expected 'share,m:key1,key2,key3'".to_string());
        }

        let share_m: Vec<&str> = parts[0].split(',').collect();
        if share_m.len() != 2 {
            return Err("Invalid share,m format".to_string());
        }

        let share = share_m[0].parse::<u64>().map_err(|e| e.to_string())?;
        let m = share_m[1].parse::<u64>().map_err(|e| e.to_string())?;
        let keys: Vec<String> = parts[1].split(',').map(String::from).collect();

        Ok(MiningKeyConfig { share, m, keys })
    }
}

#[derive(Debug, Clone)]
pub struct MiningPkhConfig {
    pub share: u64,
    pub pkh: String,
}

impl FromStr for MiningPkhConfig {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Expected format: "share,pkh"
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != 2 {
            return Err("Invalid share,pkh format".to_string());
        }

        let share = parts[0].parse::<u64>().map_err(|e| e.to_string())?;
        let pkh = parts[1].parse::<String>().map_err(|e| e.to_string())?;

        Ok(MiningPkhConfig { share, pkh })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mining_key_config_from_str_roundtrip() {
        let cfg = MiningKeyConfig::from_str("1,2:keyA,keyB").expect("parse");
        assert_eq!(cfg.share, 1);
        assert_eq!(cfg.m, 2);
        assert_eq!(cfg.keys, vec!["keyA".to_string(), "keyB".to_string()]);
    }

    #[test]
    fn mining_key_config_rejects_bad_format() {
        assert!(MiningKeyConfig::from_str("no-colon").is_err());
        assert!(MiningKeyConfig::from_str("just,one:keys").is_err());
    }

    #[test]
    fn mining_pkh_config_from_str_roundtrip() {
        let cfg = MiningPkhConfig::from_str("3,base58hash").expect("parse");
        assert_eq!(cfg.share, 3);
        assert_eq!(cfg.pkh, "base58hash");
    }

    #[test]
    fn mining_pkh_config_rejects_bad_format() {
        assert!(MiningPkhConfig::from_str("nocomma").is_err());
        assert!(MiningPkhConfig::from_str("notanumber,hash").is_err());
    }
}
