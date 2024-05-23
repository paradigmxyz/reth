//! Trait abstractions used by the payload crate.

use crate::error::PayloadBuilderError;
use reth_engine_primitives::{BuiltPayload, PayloadBuilderAttributes};
use reth_provider::CanonStateNotification;
use std::future::Future;
