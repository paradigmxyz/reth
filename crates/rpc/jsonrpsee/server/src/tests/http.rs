// Copyright 2019-2021 Parity Technologies (UK) Ltd.
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

use std::net::SocketAddr;

use crate::{
    BatchRequestConfig, RegisterMethodError, RpcModule, ServerBuilder, ServerConfig, ServerHandle,
};
use jsonrpsee_core::RpcResult;
use jsonrpsee_test_utils::{
    helpers::*,
    mocks::{Id, StatusCode},
    TimeoutFutureExt,
};
use jsonrpsee_types::ErrorObjectOwned;
use serde_json::Value as JsonValue;

use super::helpers::{MyAppError, TestContext};

fn init_logger() {
    let _ = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();
}

async fn server() -> (SocketAddr, ServerHandle) {
    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let ctx = TestContext;
    let mut module = RpcModule::new(ctx);
    let addr = server.local_addr().unwrap();
    module.register_method("say_hello", |_, _| "lo").unwrap();
    module
        .register_async_method("say_hello_async", |_, _| async move { RpcResult::Ok("lo") })
        .unwrap();
    module
        .register_method("add", |params, _| {
            let params: Vec<u64> = params.parse()?;
            let sum: u64 = params.into_iter().sum();
            RpcResult::Ok(sum)
        })
        .unwrap();
    module
        .register_method::<Result<String, ErrorObjectOwned>, _>("multiparam", |params, _| {
            let params: (String, String, Vec<u8>) = params.parse()?;
            let r = format!(
                "string1={}, string2={}, vec={}",
                params.0.len(),
                params.1.len(),
                params.2.len()
            );
            Ok(r)
        })
        .unwrap();
    module.register_method("notif", |_, _| "").unwrap();
    module
        .register_method("should_err", |_, ctx| {
            ctx.err()?;
            Ok::<_, MyAppError>("err")
        })
        .unwrap();

    module
        .register_method("should_ok", |_, ctx| {
            ctx.ok()?;
            Ok::<_, MyAppError>("ok")
        })
        .unwrap();
    module
        .register_async_method("should_ok_async", |_p, ctx| async move {
            ctx.ok()?;
            Ok::<_, MyAppError>("ok")
        })
        .unwrap();

    let server_handle = server.start(module);
    (addr, server_handle)
}

#[tokio::test]
async fn single_method_call_works() {
    init_logger();
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    for i in 0..10 {
        let req = format!(r#"{{"jsonrpc":"2.0","method":"say_hello","id":{i}}}"#);
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(i)));
    }
}

#[tokio::test]
async fn async_method_call_works() {
    init_logger();
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    for i in 0..10 {
        let req = format!(r#"{{"jsonrpc":"2.0","method":"say_hello_async","id":{i}}}"#);
        let response = http_request(req.into(), uri.clone()).await.unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(i)));
    }
}

#[tokio::test]
async fn invalid_single_method_call() {
    init_logger();
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":1, "params": "bar"}"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_request(Id::Null));
}

#[tokio::test]
async fn single_method_call_with_params() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response(JsonValue::Number(3.into()), Id::Num(1)));
}

#[tokio::test]
async fn single_method_call_with_multiple_params_of_different_types() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req =
        r#"{"jsonrpc":"2.0","method":"multiparam", "params":["Hello", "World", [0,1,2,3]],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.body,
        ok_response(JsonValue::String("string1=5, string2=5, vec=4".into()), Id::Num(1))
    );
}

#[tokio::test]
async fn single_method_call_with_faulty_params_returns_err() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    let expected = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Invalid params","data":"invalid type: string \"this should be a number\", expected u64 at line 1 column 26"}}"#;

    let req = r#"{"jsonrpc":"2.0","method":"add", "params":["this should be a number"],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);
}

#[tokio::test]
async fn single_method_call_with_faulty_context() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"should_err","params":[],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, call_execution_failed("MyAppError", Id::Num(1)));
}

#[tokio::test]
async fn single_method_call_with_ok_context() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"should_ok", "params":[],"id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response("ok".into(), Id::Num(1)));
}

#[tokio::test]
async fn async_method_call_with_ok_context() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"should_ok_async", "params":[],"id":1}"#;
    let response = http_request(req.into(), uri).await.unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response("ok".into(), Id::Num(1)));
}

#[tokio::test]
async fn valid_batched_method_calls() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"[
        {"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1},
        {"jsonrpc":"2.0","method":"add", "params":[3, 4],"id":2},
        {"jsonrpc":"2.0","method":"say_hello","id":3},
        {"jsonrpc":"2.0","method":"add", "params":[5, 6],"id":4}
    ]"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.body,
        r#"[{"jsonrpc":"2.0","id":1,"result":3},{"jsonrpc":"2.0","id":2,"result":7},{"jsonrpc":"2.0","id":3,"result":"lo"},{"jsonrpc":"2.0","id":4,"result":11}]"#
    );
}

#[tokio::test]
async fn batched_notifications() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"[{"jsonrpc": "2.0", "method": "notif", "params": [1,2,4]},{"jsonrpc": "2.0", "method": "notif", "params": [7]}]"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    // Note: on HTTP acknowledge the notification with an empty response.
    assert_eq!(response.body, "null");
}

#[tokio::test]
async fn invalid_batch_calls() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    // batch with no requests
    let req = r#"[]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_request(Id::Null));

    // batch with invalid request
    let req = r#"[123]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_batch(vec![Id::Null]));

    // batch with invalid request
    let req = r#"[1, 2, 3]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_batch(vec![Id::Null, Id::Null, Id::Null]));

    // invalid JSON in batch
    let req = r#"[
        {"jsonrpc": "2.0", "method": "sum", "params": [1,2,4], "id": "1"},
        {"jsonrpc": "2.0", "method"
      ]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn batch_with_mixed_calls() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    // mixed notifications, method calls and valid json should be valid.
    let req = r#"[
            {"jsonrpc": "2.0", "method": "add", "params": [1,2,4], "id": "1"},
            {"jsonrpc": "2.0", "method": "add", "params": [7]},
            {"foo": "boo"},
            {"jsonrpc": "2.0", "method": "foo.get", "params": {"name": "myself"}, "id": "5"}
        ]"#;
    let res = r#"[{"jsonrpc":"2.0","id":"1","result":7},{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request"}},{"jsonrpc":"2.0","id":"5","error":{"code":-32601,"message":"Method not found"}}]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, res);
}

#[tokio::test]
async fn batch_notif_without_params_works() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    // mixed notifications, method calls and valid json should be valid.
    let req = r#"[
            {"jsonrpc": "2.0", "method": "add", "params": [1,2,4], "id": "1"},
            {"jsonrpc": "2.0", "method": "add"}
        ]"#;
    let res = r#"[{"jsonrpc":"2.0","id":"1","result":7}]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, res);
}

#[tokio::test]
async fn garbage_request_fails() {
    let (addr, _handle) = server().await;
    let uri = to_http_uri(addr);

    let req = r#"dsdfs fsdsfds"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"{ "#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"{}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, invalid_request(Id::Null));

    let req = r#"{sds}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"["#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"[dsds]"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));

    let req = r#"[]"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, invalid_request(Id::Null));

    let req = r#"[{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn whitespace_is_not_significant() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"         {"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    let expected = r#"{"jsonrpc":"2.0","id":1,"result":3}"#;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);

    let req = r#" [{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}]"#;
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    let expected = r#"[{"jsonrpc":"2.0","id":1,"result":3}]"#;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);

    // Up to 127 whitespace chars are accepted.
    let req = format!(
        "{}{}",
        " ".repeat(127),
        r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#
    );
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    let expected = r#"{"jsonrpc":"2.0","id":1,"result":3}"#;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, expected);

    // More than 127 whitespace chars are not accepted.
    let req = format!(
        "{}{}",
        " ".repeat(128),
        r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#
    );
    let response = http_request(req.into(), uri.clone()).await.unwrap();
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn should_return_method_not_found() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"bar","id":"foo"}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, method_not_found(Id::Str("foo".into())));
}

#[tokio::test]
async fn invalid_json_id_missing_value() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"say_hello","id"}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    // If there was an error in detecting the id in the Request object (e.g. Parse error/Invalid
    // Request), it MUST be Null.
    assert_eq!(response.body, parse_error(Id::Null));
}

#[tokio::test]
async fn shared_server_behavior_http_and_websocket_invalid_envelopes() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);
    for req in [r#"{"jsonrpc":"1.0","method":"say_hello"}"#, r#"{"jsonrpc":"2.0"}"#, r#"{}"#] {
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, invalid_request(Id::Null));
        let response =
            http_request(r#"{"jsonrpc":"2.0","method":"say_hello","id":1}"#.into(), uri.clone())
                .with_default_timeout()
                .await
                .unwrap()
                .unwrap();
        assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(1)));
    }
}

#[tokio::test]
async fn invalid_request_object() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"method":"bar","id":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, invalid_request(Id::Num(1)));
}

#[tokio::test]
async fn unknown_field_is_ok() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"say_hello","id":1,"is_not_request_object":1}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response(JsonValue::String("lo".to_owned()), Id::Num(1)));
}

#[tokio::test]
async fn notif_works() {
    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"bar"}"#;
    let response = http_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, "null");
}

#[tokio::test]
async fn can_register_modules() {
    let cx = String::new();
    let mut mod1 = RpcModule::new(cx);

    let cx2 = Vec::<u8>::new();
    let mut mod2 = RpcModule::new(cx2);

    assert_eq!(mod1.method_names().count(), 0);
    mod1.register_method("bla", |_, cx| format!("Gave me {cx}")).unwrap();
    mod1.register_method("bla2", |_, cx| format!("Gave me {cx}")).unwrap();
    mod2.register_method("yada", |_, cx| format!("Gave me {cx:?}")).unwrap();

    // Won't register, name clashes
    mod2.register_method("bla", |_, cx| format!("Gave me {cx:?}")).unwrap();

    assert_eq!(mod1.method_names().count(), 2);

    let err = mod1.merge(mod2).unwrap_err();

    let expected_err = RegisterMethodError::AlreadyRegistered(String::from("bla"));
    assert_eq!(err.to_string(), expected_err.to_string());
    assert_eq!(mod1.method_names().count(), 2);
}

#[tokio::test]
async fn can_set_the_max_request_body_size() {
    let addr = "127.0.0.1:0";
    // Rejects all requests larger than 100 bytes
    let config = ServerConfig::builder().max_request_body_size(100).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("anything", |_p, _cx| "a".repeat(100)).unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Invalid: too long
    let req = format!(r#"{{"jsonrpc":"2.0", "method":{}, "id":1}}"#, "a".repeat(100));
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, oversized_request(100));

    // Max request body size should not override the max response size
    let req = r#"{"jsonrpc":"2.0", "method":"anything", "id":1}"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, ok_response(JsonValue::String("a".repeat(100)), Id::Num(1)));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn can_set_the_max_response_size() {
    let addr = "127.0.0.1:0";
    // Set the max response size to 100 bytes
    let config = ServerConfig::builder().max_response_body_size(100).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("anything", |_p, _cx| "a".repeat(101)).unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Oversized response.
    let req = r#"{"jsonrpc":"2.0", "method":"anything", "id":1}"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, oversized_response(Id::Num(1), 100));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn can_set_the_max_response_size_to_batch() {
    let addr = "127.0.0.1:0";
    // Set the max response size to 100 bytes
    let config = ServerConfig::builder().max_response_body_size(100).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("anything", |_p, _cx| "a".repeat(51)).unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Two response will end up in a response of 102 bytes which is too big.
    let req = r#"[{"jsonrpc":"2.0", "method":"anything", "id":1},{"jsonrpc":"2.0", "method":"anything", "id":2}]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, batch_response_too_large(100));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn disabled_batches() {
    let addr = "127.0.0.1:0";
    // Disable batches support.
    let config =
        ServerConfig::builder().set_batch_request_config(BatchRequestConfig::Disabled).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("should_ok", |_, _ctx| "ok").unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Send a valid batch.
    let req = r#"[
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":1},
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":2}
    ]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, batches_not_supported());

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn batch_limit_works() {
    let addr = "127.0.0.1:0";
    // Disable batches support.
    let config =
        ServerConfig::builder().set_batch_request_config(BatchRequestConfig::Limit(1)).build();
    let server = ServerBuilder::with_config(config).build(addr).await.unwrap();
    let mut module = RpcModule::new(());
    module.register_method("should_ok", |_, _ctx| "ok").unwrap();
    let addr = server.local_addr().unwrap();
    let uri = to_http_uri(addr);
    let handle = server.start(module);

    // Send a valid batch.
    let req = r#"[
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":1},
        {"jsonrpc":"2.0","method":"should_ok", "params":[],"id":2}
    ]"#;
    let response =
        http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.body, batches_too_large(1));

    handle.stop().unwrap();
    handle.stopped().await;
}

#[tokio::test]
async fn http2_method_call_works() {
    init_logger();

    let (addr, _handle) = server().with_default_timeout().await.unwrap();
    let uri = to_http_uri(addr);

    let req = r#"{"jsonrpc":"2.0","method":"add", "params":[1, 2],"id":1}"#;
    let response = http2_request(req.into(), uri).with_default_timeout().await.unwrap().unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, ok_response(JsonValue::Number(3.into()), Id::Num(1)));
}

#[tokio::test]
async fn notification_id_classification_http_singles_and_mixed_batches() {
    use serde_json::json;
    let (addr, handle, calls) = super::helpers::server_with_call_counter().await;
    let uri = to_http_uri(addr);
    let error: JsonValue = serde_json::from_str(&invalid_request(Id::Null)).unwrap();
    for id in [json!(true), json!(false), json!({}), json!([])] {
        let invalid = json!({"jsonrpc":"2.0","method":"tick","id":id});
        for batch in [false, true] {
            let before = calls.load(std::sync::atomic::Ordering::SeqCst);
            let request = if batch {
                json!([invalid, {"jsonrpc":"2.0","method":"tick","id":"okay"}, {"jsonrpc":"2.0","method":"tick","id":null}])
            } else {
                invalid.clone()
            };
            let response = http_request(request.to_string().into(), uri.clone())
                .with_default_timeout()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.status, StatusCode::OK);
            let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
            let expected = if batch {
                json!([error, {"jsonrpc":"2.0","id":"okay","result":before}, {"jsonrpc":"2.0","id":null,"result":before + 1}])
            } else {
                error.clone()
            };
            assert_eq!(reply, expected, "invalid_id_error_missing");
            let after = before + if batch { 2 } else { 0 };
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::SeqCst),
                after,
                "invalid ID dispatched"
            );
            let request = json!({"jsonrpc":"2.0","method":"tick","id":"recovery"});
            let response = http_request(request.to_string().into(), uri.clone())
                .with_default_timeout()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(response.status, StatusCode::OK);
            let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
            assert_eq!(reply, json!({"jsonrpc":"2.0","id":"recovery","result":after}));
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), after + 1);
        }
    }
    for id in [json!(0), json!("valid"), JsonValue::Null] {
        let before = calls.load(std::sync::atomic::Ordering::SeqCst);
        let request = json!({"jsonrpc":"2.0","method":"tick","id":id});
        let response = http_request(request.to_string().into(), uri.clone())
            .with_default_timeout()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status, StatusCode::OK);
        let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
        assert_eq!(reply, json!({"jsonrpc":"2.0","id":id,"result":before}));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), before + 1);
    }
    let request = json!({"jsonrpc":"2.0","method":"tick","extra":{"id":true}});
    let response = http_request(request.to_string().into(), uri.clone())
        .with_default_timeout()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, "null");
    let request = json!({"jsonrpc":"2.0","method":"tick","id":"after-notification"});
    let response = http_request(request.to_string().into(), uri)
        .with_default_timeout()
        .await
        .unwrap()
        .unwrap();
    let reply: JsonValue = serde_json::from_str(&response.body).unwrap();
    assert_eq!(reply["id"], "after-notification");
    assert!(reply.get("result").is_some());
    assert!(reply.get("error").is_none());
    handle.stop().unwrap();
    handle.stopped().with_default_timeout().await.unwrap();
}

#[tokio::test]
async fn panicking_call_returns_internal_error() {
    init_logger();

    let server = ServerBuilder::default().build("127.0.0.1:0").await.unwrap();
    let mut module = RpcModule::new(());
    module
        .register_async_method("maybe_panic", |params, _| async move {
            assert!(!params.one::<bool>().unwrap(), "requested panic");
            "ok"
        })
        .unwrap();
    module
        .register_method("maybe_panic_sync", |params, _| {
            assert!(!params.one::<bool>().unwrap(), "requested panic");
            "ok"
        })
        .unwrap();
    let uri = to_http_uri(server.local_addr().unwrap());
    let handle = server.start(module);

    for method in ["maybe_panic", "maybe_panic_sync"] {
        let req = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":[true],"id":1}}"#);
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body, internal_error(Id::Num(1)));

        let req = format!(
            r#"[
                {{"jsonrpc":"2.0","method":"{method}","params":[true],"id":1}},
                {{"jsonrpc":"2.0","method":"{method}","params":[false],"id":2}}
            ]"#
        );
        let response =
            http_request(req.into(), uri.clone()).with_default_timeout().await.unwrap().unwrap();
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(
            response.body,
            format!("[{},{}]", internal_error(Id::Num(1)), ok_response("ok".into(), Id::Num(2)))
        );
    }

    handle.stop().unwrap();
    handle.stopped().await;
}
