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
    }
}

macro_rules! cfg_server_core {
    ($($item:item)*) => {
        cfg_feature!("server-core", $($item)*);
    }
}

macro_rules! cfg_proc_macros {
    ($($item:item)*) => {
        cfg_feature!("jsonrpsee-proc-macros", $($item)*);
    };
}

macro_rules! cfg_types {
    ($($item:item)*) => {
        cfg_feature!("jsonrpsee-types", $($item)*);
    };
}
