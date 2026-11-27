use jsonrpsee_types::ErrorObjectOwned;

// We're marking functions on the error paths as #[cold] to both reduce chance of inlining and to
// make the generated assembly slightly better.

#[cold]
pub fn log_fail_parse(arg_pat: &str, ty: &str, err: &ErrorObjectOwned, optional: bool) {
    let optional = if optional { "optional " } else { "" };
    tracing::debug!("Error parsing {optional}\"{arg_pat}\" as \"{ty}\": {err}");
}

#[cfg(debug_assertions)]
#[cold]
pub fn panic_fail_register() -> ! {
    panic!("RPC macro method names should never conflict. This is a bug, please report it.");
}
