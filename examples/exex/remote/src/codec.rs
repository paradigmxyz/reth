use reth_exex::ExExNotification;

pub unsafe fn to_u8_slice(notification: &ExExNotification) -> &[u8] {
    core::slice::from_raw_parts(
        (notification as *const ExExNotification) as *const u8,
        core::mem::size_of::<ExExNotification>(),
    )
}

pub unsafe fn from_u8_slice(bytes: &[u8]) -> &ExExNotification {
    unsafe { &*(bytes.as_ptr() as *const reth_exex::ExExNotification) }
}
