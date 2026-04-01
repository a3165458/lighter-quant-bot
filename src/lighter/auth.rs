use super::error::LighterError;
use super::ffi;

/// Create an authentication token for Lighter authenticated endpoints.
/// `deadline_secs` is the Unix timestamp (seconds) when the token expires.
#[allow(dead_code)]
pub fn create_auth_token(deadline_secs: i64) -> Result<String, LighterError> {
    ffi::create_auth_token(deadline_secs)
}

#[cfg(test)]
mod tests {
    // Auth tests require the FFI signer to be initialized,
    // which needs the .so library — skipped in unit tests.
}
