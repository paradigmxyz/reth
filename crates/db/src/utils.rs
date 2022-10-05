pub const TEMP_PLACEHOLDER_NUM_TABLES: usize = 2;

/// Returns the default page size that can be used in this OS.
pub fn default_page_size() -> usize {
    let os_page_size = page_size::get();
    let libmdbx_max_page_size = 0x10000;

    if os_page_size < 4096 {
        // may lead to errors if it's reduced further because of the potential size of the data
        4096
    } else if os_page_size > libmdbx_max_page_size {
        libmdbx_max_page_size
    } else {
        os_page_size
    }
}
