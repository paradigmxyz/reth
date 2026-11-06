macro_rules! cfg_feature {
    ($feature:literal, $($item:item)*) => {
        $(
            #[cfg(feature = $feature)]
            #[cfg_attr(docsrs, doc(cfg(feature = $feature)))]
            $item
        )*
}
}

macro_rules! cfg_server {
 ($($item:item)*) => {
        cfg_feature!("server", $($item)*);
    };
}

macro_rules! cfg_http_helpers {
 ($($item:item)*) => {
        cfg_feature!("http-helpers", $($item)*);
    };
}
