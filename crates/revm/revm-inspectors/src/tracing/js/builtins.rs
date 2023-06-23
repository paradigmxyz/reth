//! Builtin functions

use boa_engine::{
    object::builtins::{JsArray, JsArrayBuffer},
    property::Attribute,
    Context, JsArgs, JsError, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source,
};
use boa_gc::{empty_trace, Finalize, Trace};
use reth_primitives::{
    contract::{create2_address_from_code, create_address},
    hex, keccak256, Address, H256, U256,
};
use std::collections::HashSet;

/// bigIntegerJS is the minified version of <https://github.com/peterolson/BigInteger.js>.
pub(crate) const BIG_INT_JS: &str = include_str!("bigint.js");

/// Registers all the builtin functions and global bigint property
pub(crate) fn register_builtins(ctx: &mut Context<'_>) -> JsResult<()> {
    let big_int = ctx.eval(Source::from_bytes(BIG_INT_JS.as_bytes()))?;
    ctx.register_global_property("bigint", big_int, Attribute::all())?;
    ctx.register_global_builtin_callable("toHex", 1, NativeFunction::from_fn_ptr(to_hex))?;
    ctx.register_global_callable("toWord", 1, NativeFunction::from_fn_ptr(to_word))?;
    ctx.register_global_callable("toAddress", 1, NativeFunction::from_fn_ptr(to_address))?;
    ctx.register_global_callable("toContract", 2, NativeFunction::from_fn_ptr(to_contract))?;
    ctx.register_global_callable("toContract2", 3, NativeFunction::from_fn_ptr(to_contract2))?;

    // TODO: isPrecompiled slice

    Ok(())
}

/// Converts an array, hex string or Uint8Array to a []byte
pub(crate) fn from_buf(val: JsValue, context: &mut Context<'_>) -> JsResult<Vec<u8>> {
    if let Some(obj) = val.as_object().cloned() {
        if obj.is_array_buffer() {
            let buf = JsArrayBuffer::from_object(obj)?;
            return buf.take()
        } else if obj.is_string() {
            let js_string = obj.borrow().as_string().unwrap();
            return hex_decode_js_string(js_string)
        } else if obj.is_array() {
            let array = JsArray::from_object(obj)?;
            let len = array.length(context)?;
            let mut buf = Vec::with_capacity(len as usize);
            for i in 0..len {
                let val = array.get(i, context)?;
                buf.push(val.to_number(context)? as u8);
            }
            return Ok(buf)
        }
    }

    Err(JsError::from_native(JsNativeError::typ().with_message("invalid buffer type")))
}

/// Create a new array buffer from the address' bytes.
pub(crate) fn address_to_buf(addr: Address, context: &mut Context<'_>) -> JsResult<JsArrayBuffer> {
    to_buf(addr.0.to_vec(), context)
}

/// Create a new array buffer from byte block.
pub(crate) fn to_buf(bytes: Vec<u8>, context: &mut Context<'_>) -> JsResult<JsArrayBuffer> {
    JsArrayBuffer::from_byte_block(bytes, context)
}

/// Create a new array buffer object from byte block.
pub(crate) fn to_buf_value(bytes: Vec<u8>, context: &mut Context<'_>) -> JsResult<JsValue> {
    Ok(to_buf(bytes, context)?.into())
}

/// Create a new array buffer object from byte block.
pub(crate) fn to_bigint_array(items: &[U256], ctx: &mut Context<'_>) -> JsResult<JsArray> {
    let arr = JsArray::new(ctx);
    let bigint = ctx.global_object().get("bigint", ctx)?;
    if !bigint.is_callable() {
        return Err(JsError::from_native(
            JsNativeError::typ().with_message("global object bigint is not callable"),
        ))
    }
    let bigint = bigint.as_callable().unwrap();

    for item in items {
        let val = bigint.call(&JsValue::undefined(), &[JsValue::from(item.to_string())], ctx)?;
        arr.push(val, ctx)?;
    }
    Ok(arr)
}

/// Converts a buffer type to an address.
///
/// If the buffer is larger than the address size, it will be cropped from the left
pub(crate) fn bytes_to_address(buf: Vec<u8>) -> Address {
    let mut address = Address::default();
    let mut buf = &buf[..];
    let address_len = address.0.len();
    if buf.len() > address_len {
        // crop from left
        buf = &buf[buf.len() - address.0.len()..];
    }
    let address_slice = &mut address.0[address_len - buf.len()..];
    address_slice.copy_from_slice(buf);
    address
}

/// Converts a buffer type to a hash.
///
/// If the buffer is larger than the hash size, it will be cropped from the left
pub(crate) fn bytes_to_hash(buf: Vec<u8>) -> H256 {
    let mut hash = H256::default();
    let mut buf = &buf[..];
    let hash_len = hash.0.len();
    if buf.len() > hash_len {
        // crop from left
        buf = &buf[buf.len() - hash.0.len()..];
    }
    let hash_slice = &mut hash.0[hash_len - buf.len()..];
    hash_slice.copy_from_slice(buf);
    hash
}

/// Converts a U256 to a bigint using the global bigint property
pub(crate) fn to_bigint(value: U256, ctx: &mut Context<'_>) -> JsResult<JsValue> {
    let bigint = ctx.global_object().get("bigint", ctx)?;
    if !bigint.is_callable() {
        return Ok(JsValue::undefined())
    }
    bigint.as_callable().unwrap().call(
        &JsValue::undefined(),
        &[JsValue::from(value.to_string())],
        ctx,
    )
}
/// Takes three arguments: a JavaScript value that represents the sender's address, a string salt
/// value, and the initcode for the contract. Compute the address of a contract created by the
/// sender with the given salt and code hash, then converts the resulting address back into a byte
/// buffer for output.
pub(crate) fn to_contract2(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context<'_>,
) -> JsResult<JsValue> {
    // Extract the sender's address, salt and initcode from the arguments
    let from = args.get_or_undefined(0).clone();
    let salt = match args.get_or_undefined(1).to_string(ctx) {
        Ok(js_string) => {
            let buf = hex_decode_js_string(js_string)?;
            bytes_to_hash(buf)
        }
        Err(_) => {
            return Err(JsError::from_native(JsNativeError::typ().with_message("invalid salt type")))
        }
    };

    let initcode = args.get_or_undefined(2).clone();

    // Convert the sender's address to a byte buffer and then to an Address
    let buf = from_buf(from, ctx)?;
    let addr = bytes_to_address(buf);

    // Convert the initcode to a byte buffer
    let code_buf = from_buf(initcode, ctx)?;
    // Compute the code hash
    let code_hash = keccak256(code_buf);

    // Compute the contract address
    let contract_addr = create2_address_from_code(addr, salt, code_hash.into());

    // Convert the contract address to a byte buffer and return it as an ArrayBuffer
    to_buf_value(contract_addr.0.to_vec(), ctx)
}

///  Converts the sender's address to a byte buffer
pub(crate) fn to_contract(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context<'_>,
) -> JsResult<JsValue> {
    // Extract the sender's address and nonce from the arguments
    let from = args.get_or_undefined(0).clone();
    let nonce = args.get_or_undefined(1).to_number(ctx)? as u64;

    // Convert the sender's address to a byte buffer and then to an Address
    let buf = from_buf(from, ctx)?;
    let addr = bytes_to_address(buf);

    // Compute the contract address
    let contract_addr = create_address(addr, nonce);

    // Convert the contract address to a byte buffer and return it as an ArrayBuffer
    to_buf_value(contract_addr.0.to_vec(), ctx)
}

/// Converts a buffer type to an address
pub(crate) fn to_address(
    _: &JsValue,
    args: &[JsValue],
    ctx: &mut Context<'_>,
) -> JsResult<JsValue> {
    let val = args.get_or_undefined(0).clone();
    let buf = from_buf(val, ctx)?;
    let address = bytes_to_address(buf);
    to_buf_value(address.0.to_vec(), ctx)
}

/// Converts a buffer type to a word
pub(crate) fn to_word(_: &JsValue, args: &[JsValue], ctx: &mut Context<'_>) -> JsResult<JsValue> {
    let val = args.get_or_undefined(0).clone();
    let buf = from_buf(val, ctx)?;
    let hash = bytes_to_hash(buf);
    to_buf_value(hash.0.to_vec(), ctx)
}

/// Converts a buffer type to a hex string
pub(crate) fn to_hex(_: &JsValue, args: &[JsValue], ctx: &mut Context<'_>) -> JsResult<JsValue> {
    let val = args.get_or_undefined(0).clone();
    let buf = from_buf(val, ctx)?;
    Ok(JsValue::from(hex::encode(buf)))
}

/// Decodes a hex decoded js-string
fn hex_decode_js_string(js_string: JsString) -> JsResult<Vec<u8>> {
    match js_string.to_std_string() {
        Ok(s) => match hex::decode(s.as_str()) {
            Ok(data) => Ok(data),
            Err(err) => Err(JsError::from_native(
                JsNativeError::error().with_message(format!("invalid hex string {s}: {err}",)),
            )),
        },
        Err(err) => Err(JsError::from_native(
            JsNativeError::error()
                .with_message(format!("invalid utf8 string {js_string:?}: {err}",)),
        )),
    }
}

/// A container for all precompile addresses used for the `isPrecompiled` global callable.
#[derive(Debug, Clone)]
pub(crate) struct PrecompileList(pub(crate) HashSet<Address>);

impl PrecompileList {
    /// Registers the global callable `isPrecompiled`
    pub(crate) fn register_callable(self, ctx: &mut Context<'_>) -> JsResult<()> {
        let is_precompiled = NativeFunction::from_copy_closure_with_captures(
            move |_this, args, precompiles, ctx| {
                let val = args.get_or_undefined(0).clone();
                let buf = from_buf(val, ctx)?;
                let addr = bytes_to_address(buf);
                Ok(precompiles.0.contains(&addr).into())
            },
            self,
        );

        ctx.register_global_callable("isPrecompiled", 1, is_precompiled)?;

        Ok(())
    }
}

impl Finalize for PrecompileList {}

unsafe impl Trace for PrecompileList {
    empty_trace!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::Source;

    #[test]
    fn test_install_bigint() {
        let mut ctx = Context::default();
        let big_int = ctx.eval(Source::from_bytes(BIG_INT_JS.as_bytes())).unwrap();
        let value = JsValue::from(100);
        let result =
            big_int.as_callable().unwrap().call(&JsValue::undefined(), &[value], &mut ctx).unwrap();
        assert_eq!(result.to_string(&mut ctx).unwrap().to_std_string().unwrap(), "100");
    }
}
