//! Builtin functions

// toHex, toWord, toAddress toContract toContract2 isPrecompiled slice

use boa_engine::{
    object::builtins::{JsArray, JsArrayBuffer},
    Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction,
};
use reth_primitives::U256;

/// bigIntegerJS is the minified version of https://github.com/peterolson/BigInteger.js.
pub(crate) const BIG_INT_JS: &str = include_str!("bigint.js");

pub(crate) fn register_builtins(ctx: &mut Context<'_>) -> JsResult<()> {
    ctx.register_global_builtin_callable("toHex", 1, NativeFunction::from_fn_ptr(to_hex))?;

    Ok(())
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

// func toBuf(vm *goja.Runtime, bufType goja.Value, val []byte) (goja.Value, error) {
// // bufType is usually Uint8Array. This is equivalent to `new Uint8Array(val)` in JS.
// return vm.New(bufType, vm.ToValue(vm.NewArrayBuffer(val)))
// }

pub(crate) fn to_hex(_: &JsValue, _args: &[JsValue], _ctx: &mut Context<'_>) -> JsResult<JsValue> {
    todo!()
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
