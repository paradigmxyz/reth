macro_rules! cfg_feature {
	($feature:literal, $($item:item)*) => {
		$(
			#[cfg(feature = $feature)]
			#[cfg_attr(docsrs, doc(cfg(feature = $feature)))]
			$item
		)*
}
}

macro_rules! cfg_client {
	($($item:item)*) => {
		cfg_feature!("client", $($item)*);
	};
}

macro_rules! cfg_server {
 ($($item:item)*) => {
		cfg_feature!("server", $($item)*);
	};
}

macro_rules! cfg_client_or_server {
	($($item:item)*) => {
		$(
			#[cfg(any(feature = "client", feature = "server"))]
			$item
		)*
	}
}

macro_rules! cfg_http_helpers {
 ($($item:item)*) => {
		cfg_feature!("http-helpers", $($item)*);
	};
}

macro_rules! cfg_async_client {
	($($item:item)*) => {
		$(
			#[cfg(any(feature = "async-wasm-client", feature = "async-client"))]
			#[cfg_attr(docsrs, doc(cfg(feature = "async-client")))]
			#[cfg_attr(docsrs, doc(cfg(feature = "async-wasm-client")))]
			$item
		)*
	}
}
