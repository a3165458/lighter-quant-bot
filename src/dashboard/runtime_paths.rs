use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

pub fn data_file(network: &str, filename: &str) -> Result<PathBuf> {
    if !matches!(network, "mainnet" | "robinhood") {
        bail!("unsupported runtime data network: {network}");
    }
    let leaf = Path::new(filename);
    if leaf.file_name().and_then(|value| value.to_str()) != Some(filename) {
        bail!("runtime data filename must be a plain leaf name: {filename}");
    }
    Ok(PathBuf::from("data").join(network).join(filename))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn runtime_files_are_isolated_by_network() {
        assert_eq!(
            data_file("mainnet", "pnl_state.json").unwrap(),
            PathBuf::from("data/mainnet/pnl_state.json")
        );
        assert_eq!(
            data_file("robinhood", "strategy_config.json").unwrap(),
            PathBuf::from("data/robinhood/strategy_config.json")
        );
    }

    #[test]
    fn invalid_network_names_cannot_escape_the_data_directory() {
        assert!(data_file("../mainnet", "pnl_state.json").is_err());
        assert!(data_file("testnet", "pnl_state.json").is_err());
    }

    #[test]
    fn filenames_must_be_plain_leaf_names() {
        assert!(data_file("mainnet", "../pnl_state.json").is_err());
        assert!(data_file("mainnet", "nested/pnl_state.json").is_err());
    }
}
