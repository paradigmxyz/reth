# RETH network implementation

Supports both devp2p peer discovery version 4 and 5. The default is Discv4. Discv5 
is available as a feature. Behind the scenes, Discv5 runs Discv4 too, for 
connecting to peers that don't connect over Discv5.