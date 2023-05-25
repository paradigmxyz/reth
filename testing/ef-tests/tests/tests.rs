#![cfg(feature = "ef-tests")]

use crate::cases::GeneralStateTests;

#[test]
fn general_state_tests() {
    GeneralStateTests::default().run();
}
