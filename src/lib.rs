pub mod core;
pub mod tftpcl_util;
pub mod tftpd_util;

#[macro_export]
macro_rules! elog {
    ($($expr:tt)*) => {{
        /* For, now, just print to stderr. */
        eprintln!($($expr)*);
    }};
}

#[macro_export]
macro_rules! elog_fatal {
    ($exit_status:expr, $($expr:tt)*) => {{
        eprintln!($($expr)*);
        std::process::exit($exit_status);
    }};
}
