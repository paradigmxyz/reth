# RETH network implementation

Supports both devp2p peer discovery version 4  5. The default is Discv4 and is 
enabled as default feature for the crate. Behind the scenes, Discv5 runs with 
support for downgrading to Discv4. The feature flags, `discv5` and `discv4`, 
are mutually exclusive.