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

use std::time::Duration;

use crate::HttpClientBuilder;
use crate::types::error::{ErrorCode, ErrorObject};
use jsonrpsee_core::ClientError;
use jsonrpsee_core::client::{BatchResponse, ClientT, IdKind};
use jsonrpsee_core::params::BatchRequestBuilder;
use jsonrpsee_core::{DeserializeOwned, rpc_params};
use jsonrpsee_test_utils::TimeoutFutureExt;
use jsonrpsee_test_utils::helpers::*;
use jsonrpsee_test_utils::mocks::Id;
use jsonrpsee_types::error::ErrorObjectOwned;

fn init_logger() {
	let _ = tracing_subscriber::FmtSubscriber::builder()
		.with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
		.try_init();
}

#[tokio::test]
async fn method_call_works() {
	let result = run_request_with_response(ok_response("hello".into(), Id::Num(0)))
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();
	assert_eq!("hello", &result);
}

#[tokio::test]
async fn connect_timeout_fires_before_request_timeout() {
	// 192.0.2.1 is in TEST-NET-1 (RFC 5737): reserved and not routable, so the TCP handshake never
	// completes. With a short connect timeout the request must fail quickly instead of waiting out the
	// much longer request timeout — exactly what would regress if `connect_timeout` were not wired
	// through to the connector.
	let client = HttpClientBuilder::default()
		.connect_timeout(Duration::from_millis(100))
		.request_timeout(Duration::from_secs(60))
		.build("http://192.0.2.1:9")
		.unwrap();

	// Bound the test well under the 60s request timeout: the connect should fail in ~100ms. If it
	// regressed and hung until the request timeout, this outer 10s bound trips and fails the test.
	let res = tokio::time::timeout(Duration::from_secs(10), client.request::<String, _>("say_hello", rpc_params![]))
		.await
		.expect("connect should fail well before the request timeout");

	match res {
		Ok(_) => panic!("expected a connect error, got a successful response"),
		Err(ClientError::RequestTimeout) => panic!("request failed via the request timeout, not the connect timeout"),
		Err(_) => {}
	}
}

#[tokio::test]
async fn method_call_with_wrong_id_kind() {
	let exp = "id as string";
	let server_addr =
		http_server_with_hardcoded_response(ok_response(exp.into(), Id::Num(0))).with_default_timeout().await.unwrap();
	let uri = format!("http://{server_addr}");
	let client = HttpClientBuilder::default().id_format(IdKind::String).build(&uri).unwrap();
	let res: Result<String, ClientError> = client.request("o", rpc_params![]).with_default_timeout().await.unwrap();
	assert!(matches!(res, Err(ClientError::InvalidRequestId(_))));
}

#[tokio::test]
async fn method_call_with_id_str() {
	let exp = "id as string";
	let server_addr = http_server_with_hardcoded_response(ok_response(exp.into(), Id::Str("0".into())))
		.with_default_timeout()
		.await
		.unwrap();
	let uri = format!("http://{server_addr}");
	let client = HttpClientBuilder::default().id_format(IdKind::String).build(&uri).unwrap();
	let response: String = client.request("o", rpc_params![]).with_default_timeout().await.unwrap().unwrap();
	assert_eq!(&response, exp);
}

#[tokio::test]
async fn notification_works() {
	let server_addr = http_server_with_hardcoded_response(String::new()).with_default_timeout().await.unwrap();
	let uri = format!("http://{server_addr}");
	let client = HttpClientBuilder::default().build(&uri).unwrap();
	client
		.notification("i_dont_care_about_the_response_because_the_server_should_not_respond", rpc_params![])
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();
}

#[tokio::test]
async fn response_with_wrong_id() {
	let err = run_request_with_response(ok_response("hello".into(), Id::Num(99)))
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap_err();
	assert!(matches!(err, ClientError::InvalidRequestId(_)));
}

#[tokio::test]
async fn response_method_not_found() {
	let err =
		run_request_with_response(method_not_found(Id::Num(0))).with_default_timeout().await.unwrap().unwrap_err();
	assert_jsonrpc_error_response(err, ErrorObject::from(ErrorCode::MethodNotFound).into_owned());
}

#[tokio::test]
async fn response_parse_error() {
	let err = run_request_with_response(parse_error(Id::Num(0))).with_default_timeout().await.unwrap().unwrap_err();
	assert_jsonrpc_error_response(err, ErrorObject::from(ErrorCode::ParseError).into_owned());
}

#[tokio::test]
async fn invalid_request_works() {
	let err =
		run_request_with_response(invalid_request(Id::Num(0_u64))).with_default_timeout().await.unwrap().unwrap_err();
	assert_jsonrpc_error_response(err, ErrorObject::from(ErrorCode::InvalidRequest).into_owned());
}

#[tokio::test]
async fn invalid_params_works() {
	let err =
		run_request_with_response(invalid_params(Id::Num(0_u64))).with_default_timeout().await.unwrap().unwrap_err();
	assert_jsonrpc_error_response(err, ErrorObject::from(ErrorCode::InvalidParams).into_owned());
}

#[tokio::test]
async fn internal_error_works() {
	let err =
		run_request_with_response(internal_error(Id::Num(0_u64))).with_default_timeout().await.unwrap().unwrap_err();
	assert_jsonrpc_error_response(err, ErrorObject::from(ErrorCode::InternalError).into_owned());
}

#[tokio::test]
async fn subscription_response_to_request() {
	let req = r#"{"jsonrpc":"2.0","method":"subscribe_hello","params":{"subscription":"3px4FrtxSYQ1zBKW154NoVnrDhrq764yQNCXEgZyM6Mu","result":"hello my friend"}}"#.to_string();
	let err = run_request_with_response(req).with_default_timeout().await.unwrap().unwrap_err();
	assert!(matches!(err, ClientError::ParseError(_)));
}

#[tokio::test]
async fn batch_request_works() {
	let mut batch_request = BatchRequestBuilder::new();
	batch_request.insert("say_hello", rpc_params![]).unwrap();
	batch_request.insert("say_goodbye", rpc_params![0_u64, 1, 2]).unwrap();
	batch_request.insert("get_swag", rpc_params![]).unwrap();
	let server_response = r#"[{"jsonrpc":"2.0","result":"hello","id":0}, {"jsonrpc":"2.0","result":"goodbye","id":1}, {"jsonrpc":"2.0","result":"here's your swag","id":2}]"#.to_string();
	let batch_response = run_batch_request_with_response::<String>(batch_request, server_response)
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();
	assert_eq!(batch_response.num_successful_calls(), 3);
	let results: Vec<String> = batch_response.into_ok().unwrap().collect();
	assert_eq!(results, vec!["hello".to_string(), "goodbye".to_string(), "here's your swag".to_string()]);
}

#[tokio::test]
async fn batch_request_with_failed_call_works() {
	let mut batch_request = BatchRequestBuilder::new();
	batch_request.insert("say_hello", rpc_params![]).unwrap();
	batch_request.insert("say_goodbye", rpc_params![0_u64, 1, 2]).unwrap();
	batch_request.insert("get_swag", rpc_params![]).unwrap();
	let server_response = r#"[{"jsonrpc":"2.0","result":"hello","id":0}, {"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":1}, {"jsonrpc":"2.0","result":"here's your swag","id":2}]"#.to_string();
	let res = run_batch_request_with_response::<String>(batch_request, server_response)
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();
	assert_eq!(res.num_successful_calls(), 2);
	assert_eq!(res.num_failed_calls(), 1);
	assert_eq!(res.len(), 3);

	let successful_calls: Vec<_> = res.iter().filter_map(|r| r.as_ref().ok()).collect();
	let failed_calls: Vec<_> = res.iter().filter_map(|r| r.clone().err()).collect();

	assert_eq!(successful_calls, vec!["hello", "here's your swag"]);
	assert_eq!(failed_calls, vec![ErrorObject::from(ErrorCode::MethodNotFound)]);
}

#[tokio::test]
async fn batch_request_with_failed_call_gives_proper_error() {
	let mut batch_request = BatchRequestBuilder::new();
	batch_request.insert("say_hello", rpc_params![]).unwrap();
	batch_request.insert("say_goodbye", rpc_params![0_u64, 1, 2]).unwrap();
	batch_request.insert("get_swag", rpc_params![]).unwrap();
	let server_response = r#"[{"jsonrpc":"2.0","result":"hello","id":0}, {"jsonrpc":"2.0","error":{"code":-32601,"message":"Method not found"},"id":1}, {"jsonrpc":"2.0","error":{"code":-32602,"message":"foo"},"id":2}]"#.to_string();
	let res = run_batch_request_with_response::<String>(batch_request, server_response)
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();
	let err: Vec<_> = res.into_ok().unwrap_err().collect();
	assert_eq!(err, vec![ErrorObject::from(ErrorCode::MethodNotFound), ErrorObject::borrowed(-32602, "foo", None)]);
}

#[tokio::test]
async fn batch_request_with_untagged_enum_works() {
	init_logger();

	#[derive(serde::Deserialize, Clone, Debug, PartialEq)]
	#[serde(untagged)]
	enum Custom {
		Text(String),
		Number(u8),
	}

	impl Default for Custom {
		fn default() -> Self {
			Self::Number(0)
		}
	}

	let mut batch_request = BatchRequestBuilder::new();
	batch_request.insert("text", rpc_params![]).unwrap();
	batch_request.insert("binary", rpc_params![0_u64, 1, 2]).unwrap();
	let server_response =
		r#"[{"jsonrpc":"2.0","result":"hello","id":0}, {"jsonrpc":"2.0","result":13,"id":1}]"#.to_string();
	let res = run_batch_request_with_response::<Custom>(batch_request, server_response)
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();

	assert_eq!(res.num_successful_calls(), 2);
	assert_eq!(res.num_failed_calls(), 0);
	assert_eq!(res.len(), 2);
	let response: Vec<_> = res.into_ok().unwrap().collect();

	assert_eq!(response, vec![Custom::Text("hello".to_string()), Custom::Number(13)]);
}

#[tokio::test]
async fn batch_request_out_of_order_response() {
	let mut batch_request = BatchRequestBuilder::new();
	batch_request.insert("say_hello", rpc_params![]).unwrap();
	batch_request.insert("say_goodbye", rpc_params![0_u64, 1, 2]).unwrap();
	batch_request.insert("get_swag", rpc_params![]).unwrap();
	let server_response = r#"[{"jsonrpc":"2.0","result":"here's your swag","id":2}, {"jsonrpc":"2.0","result":"hello","id":0}, {"jsonrpc":"2.0","result":"goodbye","id":1}]"#.to_string();
	let res = run_batch_request_with_response::<String>(batch_request, server_response)
		.with_default_timeout()
		.await
		.unwrap()
		.unwrap();
	assert_eq!(res.num_successful_calls(), 3);
	assert_eq!(res.num_failed_calls(), 0);
	assert_eq!(res.len(), 3);
	let response: Vec<_> = res.into_ok().unwrap().collect();

	assert_eq!(response, vec!["hello".to_string(), "goodbye".to_string(), "here's your swag".to_string()]);
}

async fn run_batch_request_with_response<T: Send + DeserializeOwned + std::fmt::Debug + Clone + 'static>(
	batch: BatchRequestBuilder<'_>,
	response: String,
) -> Result<BatchResponse<T>, ClientError> {
	let server_addr = http_server_with_hardcoded_response(response).with_default_timeout().await.unwrap();
	let uri = format!("http://{server_addr}");
	let client = HttpClientBuilder::default().build(&uri).unwrap();
	client.batch_request(batch).with_default_timeout().await.unwrap()
}

async fn run_request_with_response(response: String) -> Result<String, ClientError> {
	let server_addr = http_server_with_hardcoded_response(response).with_default_timeout().await.unwrap();
	let uri = format!("http://{server_addr}");
	let client = HttpClientBuilder::default().build(&uri).unwrap();
	client.request("say_hello", rpc_params![]).with_default_timeout().await.unwrap()
}

fn assert_jsonrpc_error_response(err: ClientError, exp: ErrorObjectOwned) {
	match &err {
		ClientError::Call(err) => {
			assert_eq!(err, &exp);
		}
		e => panic!("Expected error: \"{err}\", got: {e:?}"),
	};
}
