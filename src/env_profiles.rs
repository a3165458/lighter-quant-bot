use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

const LEGACY_CREDENTIAL_KEYS: [&str; 3] = [
    "LIGHTER_SECRET_KEY",
    "LIGHTER_ACCOUNT_INDEX",
    "LIGHTER_API_KEY_INDEX",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CredentialProfile {
    Mainnet,
    Robinhood,
}

impl CredentialProfile {
    pub(crate) fn network_name(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Robinhood => "robinhood",
        }
    }

    pub(crate) fn env_key(self, suffix: &str) -> String {
        format!(
            "LIGHTER_{}_{}",
            self.network_name().to_ascii_uppercase(),
            suffix
        )
    }

    pub(crate) fn env_path(self) -> PathBuf {
        PathBuf::from(".env")
    }
}

pub(crate) struct Credentials {
    pub(crate) secret_key: String,
    pub(crate) account_index: String,
    pub(crate) api_key_index: String,
}

pub(crate) fn profile_for_chain_id(chain_id: i64) -> Result<CredentialProfile> {
    match chain_id {
        304 => Ok(CredentialProfile::Mainnet),
        466324 => Ok(CredentialProfile::Robinhood),
        other => bail!("unsupported Lighter chain_id {other}; cannot select a credential profile"),
    }
}

pub(crate) fn profile_for_network(network: &str) -> Result<CredentialProfile> {
    match network {
        "mainnet" => Ok(CredentialProfile::Mainnet),
        "robinhood" => Ok(CredentialProfile::Robinhood),
        other => bail!("unsupported Lighter network {other}"),
    }
}

#[cfg(test)]
pub(crate) fn credential_env_path(chain_id: i64) -> PathBuf {
    profile_for_chain_id(chain_id)
        .map(CredentialProfile::env_path)
        .unwrap_or_else(|_| PathBuf::from(".env.unsupported"))
}

pub(crate) fn credential_env_path_for_network(network: &str) -> Result<PathBuf> {
    Ok(profile_for_network(network)?.env_path())
}

pub(crate) fn dotenv_value(contents: &str, key: &str) -> Option<String> {
    contents.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return None;
        }
        let (candidate, value) = trimmed.split_once('=')?;
        (candidate.trim() == key).then(|| value.trim().trim_matches(['\'', '"']).to_string())
    })
}

pub(crate) fn read_env_value(path: impl AsRef<Path>, key: &str) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    dotenv_value(&contents, key)
}

pub(crate) fn selected_network() -> String {
    std::env::var("LIGHTER_NETWORK")
        .ok()
        .or_else(|| read_env_value(".env", "LIGHTER_NETWORK"))
        .filter(|value| matches!(value.as_str(), "mainnet" | "robinhood"))
        .unwrap_or_else(|| "mainnet".to_string())
}

/// Load non-credential settings from `.env`. Credential-looking keys are deliberately
/// skipped so a legacy `.env` can never override a network-specific profile.
pub(crate) fn load_shared_env() -> Result<()> {
    let Ok(contents) = std::fs::read_to_string(".env") else {
        return Ok(());
    };
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, raw_value)) = trimmed.split_once('=') else {
            bail!("invalid entry in .env: expected KEY=VALUE");
        };
        let key = key.trim().trim_start_matches("export ").trim().to_string();
        let value = raw_value.trim().trim_matches(['\'', '"']).to_string();
        if !LEGACY_CREDENTIAL_KEYS.contains(&key.as_str()) && std::env::var_os(&key).is_none() {
            std::env::set_var(key, value);
        }
    }
    Ok(())
}

pub(crate) fn load_credentials(profile: CredentialProfile) -> Result<(Credentials, PathBuf)> {
    let path = profile.env_path();
    let contents = std::fs::read_to_string(&path).unwrap_or_default();
    let value = |suffix: &str| -> Result<String> {
        let key = profile.env_key(suffix);
        dotenv_value(&contents, &key)
            .or_else(|| std::env::var(&key).ok())
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("{key} is empty or missing in {}", path.display()))
    };
    Ok((
        Credentials {
            secret_key: value("SECRET_KEY")?,
            account_index: value("ACCOUNT_INDEX")?,
            api_key_index: value("API_KEY_INDEX")?,
        },
        path,
    ))
}

#[cfg(test)]
pub(crate) fn credential_env_template(label: &str) -> String {
    format!(
        "# {label}; Lighter API keys, not wallet L1 keys\n\
LIGHTER_NETWORK=mainnet\n\
LIGHTER_MAINNET_SECRET_KEY=\n\
LIGHTER_MAINNET_ACCOUNT_INDEX=\n\
LIGHTER_MAINNET_API_KEY_INDEX=\n\
LIGHTER_ROBINHOOD_SECRET_KEY=\n\
LIGHTER_ROBINHOOD_ACCOUNT_INDEX=\n\
LIGHTER_ROBINHOOD_API_KEY_INDEX=\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn chain_ids_map_to_namespaced_keys_in_one_env_file() {
        assert_eq!(credential_env_path(304), PathBuf::from(".env"));
        assert_eq!(credential_env_path(466324), PathBuf::from(".env"));
        assert_eq!(
            profile_for_chain_id(304).unwrap().env_key("SECRET_KEY"),
            "LIGHTER_MAINNET_SECRET_KEY"
        );
        assert_eq!(
            profile_for_chain_id(466324).unwrap().env_key("SECRET_KEY"),
            "LIGHTER_ROBINHOOD_SECRET_KEY"
        );
    }

    #[test]
    fn unknown_chain_ids_are_rejected() {
        let err = profile_for_chain_id(999).unwrap_err();
        assert!(err.to_string().contains("unsupported Lighter chain_id"));
    }

    #[test]
    fn network_names_map_to_the_shared_env_file() {
        assert_eq!(
            credential_env_path_for_network("mainnet").unwrap(),
            PathBuf::from(".env")
        );
        assert_eq!(
            credential_env_path_for_network("robinhood").unwrap(),
            PathBuf::from(".env")
        );
        assert!(credential_env_path_for_network("testnet").is_err());
    }

    #[test]
    fn dotenv_value_parser_ignores_comments_and_similar_keys() {
        let input =
            "# LIGHTER_NETWORK=mainnet\nLIGHTER_NETWORK = robinhood\nOTHER_LIGHTER_NETWORK=bad\n";
        assert_eq!(
            dotenv_value(input, "LIGHTER_NETWORK"),
            Some("robinhood".to_string())
        );
        assert_eq!(dotenv_value(input, "MISSING"), None);
    }

    #[test]
    fn profile_template_contains_both_namespaced_credentials() {
        let template = credential_env_template("Robinhood Chain");
        assert!(template.contains("LIGHTER_MAINNET_SECRET_KEY="));
        assert!(template.contains("LIGHTER_ROBINHOOD_SECRET_KEY="));
        assert!(template.contains("LIGHTER_NETWORK="));
    }
}
