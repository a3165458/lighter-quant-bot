use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_longlong, c_void};
use std::path::PathBuf;
use std::sync::OnceLock;

use libloading::{Library, Symbol};
use tracing::info;

use super::error::LighterError;

#[repr(C)]
struct SignedTxResponse {
    tx_type: u8,
    tx_info: *mut c_char,
    tx_hash: *mut c_char,
    message_to_sign: *mut c_char,
    err: *mut c_char,
}

#[repr(C)]
struct StrOrErr {
    str_val: *mut c_char,
    err: *mut c_char,
}

#[allow(dead_code)]
struct SignerLib {
    _lib: Library,
    create_client: unsafe extern "C" fn(*mut c_char, *mut c_char, c_int, c_int, c_longlong) -> *mut c_char,
    sign_create_order: unsafe extern "C" fn(
        c_int, c_longlong, c_longlong, c_int, c_int, c_int, c_int, c_int, c_int,
        c_longlong, c_longlong, c_int, c_int, c_longlong, c_int, c_longlong,
    ) -> SignedTxResponse,
    sign_cancel_order: unsafe extern "C" fn(c_int, c_longlong, c_longlong, c_int, c_longlong) -> SignedTxResponse,
    sign_cancel_all_orders: unsafe extern "C" fn(c_int, c_longlong, c_longlong, c_int, c_longlong) -> SignedTxResponse,
    create_auth_token: unsafe extern "C" fn(c_longlong, c_int, c_longlong) -> StrOrErr,
    free_fn: unsafe extern "C" fn(*mut c_void),
    api_key_index: i32,
    account_index: i64,
}

// SAFETY: The Go shared library functions are thread-safe (stateless after CreateClient).
unsafe impl Send for SignerLib {}
unsafe impl Sync for SignerLib {}

static SIGNER: OnceLock<SignerLib> = OnceLock::new();

#[allow(dead_code)]
fn find_library_path() -> PathBuf {
    if let Ok(path) = std::env::var("LIGHTER_SIGNER_PATH") {
        return PathBuf::from(path);
    }
    // Check next to executable
    if let Ok(exe) = std::env::current_exe() {
        let beside_exe = exe.parent().unwrap().join("lighter-signer.so");
        if beside_exe.exists() {
            return beside_exe;
        }
    }
    // Default: current directory
    PathBuf::from("./lighter-signer.so")
}

/// Initialize the FFI signer. Must be called once at startup.
#[allow(dead_code)]
pub fn init(
    url: &str,
    private_key: &str,
    chain_id: i32,
    api_key_index: i32,
    account_index: i64,
) -> Result<(), LighterError> {
    if SIGNER.get().is_some() {
        return Ok(());
    }

    let lib_path = find_library_path();
    info!("Loading lighter-signer from: {:?}", lib_path);

    unsafe {
        let lib = Library::new(&lib_path)
            .map_err(|e| LighterError::FfiError(format!("Failed to load {}: {}", lib_path.display(), e)))?;

        let create_client: Symbol<unsafe extern "C" fn(*mut c_char, *mut c_char, c_int, c_int, c_longlong) -> *mut c_char> =
            lib.get(b"CreateClient")
                .map_err(|e| LighterError::FfiError(format!("Symbol CreateClient not found: {}", e)))?;
        let sign_create_order: Symbol<unsafe extern "C" fn(
            c_int, c_longlong, c_longlong, c_int, c_int, c_int, c_int, c_int, c_int,
            c_longlong, c_longlong, c_int, c_int, c_longlong, c_int, c_longlong,
        ) -> SignedTxResponse> =
            lib.get(b"SignCreateOrder")
                .map_err(|e| LighterError::FfiError(format!("Symbol SignCreateOrder not found: {}", e)))?;
        let sign_cancel_order: Symbol<unsafe extern "C" fn(c_int, c_longlong, c_longlong, c_int, c_longlong) -> SignedTxResponse> =
            lib.get(b"SignCancelOrder")
                .map_err(|e| LighterError::FfiError(format!("Symbol SignCancelOrder not found: {}", e)))?;
        let sign_cancel_all_orders: Symbol<unsafe extern "C" fn(c_int, c_longlong, c_longlong, c_int, c_longlong) -> SignedTxResponse> =
            lib.get(b"SignCancelAllOrders")
                .map_err(|e| LighterError::FfiError(format!("Symbol SignCancelAllOrders not found: {}", e)))?;
        let create_auth_token_fn: Symbol<unsafe extern "C" fn(c_longlong, c_int, c_longlong) -> StrOrErr> =
            lib.get(b"CreateAuthToken")
                .map_err(|e| LighterError::FfiError(format!("Symbol CreateAuthToken not found: {}", e)))?;
        let free_fn: Symbol<unsafe extern "C" fn(*mut c_void)> =
            lib.get(b"Free")
                .map_err(|e| LighterError::FfiError(format!("Symbol Free not found: {}", e)))?;

        let signer = SignerLib {
            create_client: *create_client,
            sign_create_order: *sign_create_order,
            sign_cancel_order: *sign_cancel_order,
            sign_cancel_all_orders: *sign_cancel_all_orders,
            create_auth_token: *create_auth_token_fn,
            free_fn: *free_fn,
            _lib: lib,
            api_key_index,
            account_index,
        };

        // Call CreateClient to initialize the Go signer
        let c_url = CString::new(url).unwrap();
        let c_pk = CString::new(private_key).unwrap();
        let result = (signer.create_client)(
            c_url.into_raw(),
            c_pk.into_raw(),
            chain_id,
            api_key_index,
            account_index as c_longlong,
        );
        if !result.is_null() {
            let err_str = CStr::from_ptr(result).to_string_lossy().into_owned();
            (signer.free_fn)(result as *mut c_void);
            if !err_str.is_empty() {
                return Err(LighterError::FfiError(format!("CreateClient failed: {}", err_str)));
            }
        }

        SIGNER.set(signer).map_err(|_| LighterError::FfiError("Signer already initialized".into()))?;
    }

    info!("Lighter signer initialized successfully");
    Ok(())
}

fn get_signer() -> Result<&'static SignerLib, LighterError> {
    SIGNER.get().ok_or_else(|| LighterError::FfiError("Signer not initialized. Call ffi::init() first.".into()))
}

unsafe fn read_and_free(signer: &SignerLib, ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let s = CStr::from_ptr(ptr).to_string_lossy().into_owned();
    (signer.free_fn)(ptr as *mut c_void);
    if s.is_empty() { None } else { Some(s) }
}

/// Sign a create-order transaction.
/// Returns (tx_type, tx_info_hex).
pub fn sign_create_order(
    market_index: i32,
    base_amount: i64,
    price: i32,
    is_ask: bool,
    order_type: i32,
    time_in_force: i32,
    nonce: i64,
) -> Result<(u8, String), LighterError> {
    let signer = get_signer()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let client_order_index = now.as_millis() as i64;
    // Use -1 for default 28-day expiry (same as Python SDK DEFAULT_28_DAY_ORDER_EXPIRY)
    let order_expiry: i64 = -1;
    let no_integrator: i64 = 0; // NilIntegratorIndex = 0 (from lighter-go constants)

    unsafe {
        let resp = (signer.sign_create_order)(
            market_index,
            client_order_index as c_longlong,
            base_amount as c_longlong,
            price,
            if is_ask { 1 } else { 0 },
            order_type,
            time_in_force,
            0, // reduceOnly
            0, // triggerPrice
            order_expiry as c_longlong,
            no_integrator as c_longlong,
            0, // integratorTakerFee
            0, // integratorMakerFee
            nonce as c_longlong,
            signer.api_key_index,
            signer.account_index as c_longlong,
        );

        if let Some(err) = read_and_free(signer, resp.err) {
            return Err(LighterError::FfiError(format!("SignCreateOrder: {}", err)));
        }

        let tx_info = read_and_free(signer, resp.tx_info)
            .ok_or_else(|| LighterError::FfiError("SignCreateOrder returned null tx_info".into()))?;
        let _ = read_and_free(signer, resp.tx_hash);
        let _ = read_and_free(signer, resp.message_to_sign);

        Ok((resp.tx_type, tx_info))
    }
}

/// Sign a cancel-order transaction.
/// Returns (tx_type, tx_info_hex).
pub fn sign_cancel_order(
    market_index: i32,
    order_index: i64,
    nonce: i64,
) -> Result<(u8, String), LighterError> {
    let signer = get_signer()?;

    unsafe {
        let resp = (signer.sign_cancel_order)(
            market_index,
            order_index as c_longlong,
            nonce as c_longlong,
            signer.api_key_index,
            signer.account_index as c_longlong,
        );

        if let Some(err) = read_and_free(signer, resp.err) {
            return Err(LighterError::FfiError(format!("SignCancelOrder: {}", err)));
        }

        let tx_info = read_and_free(signer, resp.tx_info)
            .ok_or_else(|| LighterError::FfiError("SignCancelOrder returned null tx_info".into()))?;
        let _ = read_and_free(signer, resp.tx_hash);
        let _ = read_and_free(signer, resp.message_to_sign);

        Ok((resp.tx_type, tx_info))
    }
}

/// Sign a cancel-all-orders transaction.
/// Returns (tx_type, tx_info_hex).
#[allow(dead_code)]
pub fn sign_cancel_all_orders(
    nonce: i64,
) -> Result<(u8, String), LighterError> {
    let signer = get_signer()?;

    unsafe {
        // Pass 0 for cancelAllTime (nil = cancel immediately)
        let resp = (signer.sign_cancel_all_orders)(
            0, // timeInForce
            0, // cancelAllTime = nil (0 means cancel now)
            nonce as c_longlong,
            signer.api_key_index,
            signer.account_index as c_longlong,
        );

        if let Some(err) = read_and_free(signer, resp.err) {
            return Err(LighterError::FfiError(format!("SignCancelAllOrders: {}", err)));
        }

        let tx_info = read_and_free(signer, resp.tx_info)
            .ok_or_else(|| LighterError::FfiError("SignCancelAllOrders returned null tx_info".into()))?;
        let _ = read_and_free(signer, resp.tx_hash);
        let _ = read_and_free(signer, resp.message_to_sign);

        Ok((resp.tx_type, tx_info))
    }
}

/// Create an auth token for authenticated WebSocket/REST endpoints.
#[allow(dead_code)]
pub fn create_auth_token(deadline_secs: i64) -> Result<String, LighterError> {
    let signer = get_signer()?;

    unsafe {
        let resp = (signer.create_auth_token)(
            deadline_secs as c_longlong,
            signer.api_key_index,
            signer.account_index as c_longlong,
        );

        if let Some(err) = read_and_free(signer, resp.err) {
            return Err(LighterError::FfiError(format!("CreateAuthToken: {}", err)));
        }

        let token = read_and_free(signer, resp.str_val)
            .ok_or_else(|| LighterError::FfiError("CreateAuthToken returned null".into()))?;

        Ok(token)
    }
}
