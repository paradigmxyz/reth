//! Table traits.

use super::KVError;
use bytes::Bytes;
use reth_primitives::{Address, U256};
use std::fmt::Debug;
use reth_interfaces::db::{Table,Encode,Decode, Object};

