//! Builtin functions

use boa_engine::{
    object::builtins::{JsArray, JsArrayBuffer},
    property::Attribute,
    Context, JsArgs, JsError, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Source,
};
use reth_primitives::{hex, Address, H256, U256};

/// bigIntegerJS is the minified version of <https://github.com/peterolson/BigInteger.js>.
pub(crate) const BIG_INT_JS: &str = include_str!("bigint.js");

/// Registers all the builtin functions and global bigint property
pub(crate) fn register_builtins(ctx: &mut Context<'_>) -> JsResult<()> {
    let big_int = ctx.eval(Source::from_bytes(BIG_INT_JS.as_bytes()))?;
    ctx.register_global_property("bigint", big_int, Attribute::all())?;
    ctx.register_global_builtin_callable("toHex", 1, NativeFunction::from_fn_ptr(to_hex))?;

    // TODO: register toWord, toAddress toContract toContract2 isPrecompiled slice

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
