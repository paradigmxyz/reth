#[macro_export]
macro_rules! print_yellow {
    ($($arg:tt)*) => {{
        println!("{}", format!($($arg)*).yellow());
    }};
}

#[macro_export]
macro_rules! print_green {
    ($($arg:tt)*) => {{
        println!("{}", format!($($arg)*).green());
    }};
}

#[macro_export]
macro_rules! print_cyan_bold {
    ($($arg:tt)*) => {{
        println!("{}", format!($($arg)*).cyan().bold());
    }};
}

#[macro_export]
macro_rules! print_red_bold {
    ($($arg:tt)*) => {{
        println!("{}", format!($($arg)*).red().bold());
    }};
}
